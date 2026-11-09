//! The one listening port: accepts bridge WebSocket connections, runs the handshake of
//! `prism_core::protocol` and then relays messages between the socket and the registry.
use crate::host::Shared;
use crate::registry::{Attach, Connection, Outgoing};
use futures_util::{SinkExt, StreamExt};
use prism_core::protocol::{
    APP_PROTOCOL, AppMessage, BridgeMessage, MAX_MESSAGE_BYTES, Prover, Refusal, Session,
    check_protocol, nonce, proof, verify_proof,
};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio::time;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{WebSocketStream, accept_async_with_config};

type Socket = WebSocketStream<TcpStream>;

/// Pause after a failed accept (for example too many open files) before the next one.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(200);
/// A bridge that sent nothing, not even a pong, for this many ping intervals is gone.
/// tungstenite on the bridge answers pings in its read loop, so no protocol change is needed.
const MISSED_PONGS: u32 = 2;

/// Binds the listener; a port in use becomes a message the user can act on.
pub async fn bind(address: SocketAddr) -> Result<TcpListener, String> {
    TcpListener::bind(address).await.map_err(|error| {
        if error.kind() == io::ErrorKind::AddrInUse {
            format!(
                "Port {} on {} is already in use. Another Prism app is probably running; quit \
                 it, or set another port in the settings file.",
                address.port(),
                address.ip()
            )
        } else {
            format!("Cannot listen on {address}: {error}")
        }
    })
}

pub(crate) async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut next_connection = 0;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            time::sleep(ACCEPT_RETRY_DELAY).await;
            continue;
        };
        // Over the limit of handshakes in progress, the connection is dropped right away; a
        // real bridge retries with backoff.
        let Ok(permit) = shared.handshakes.clone().try_acquire_owned() else {
            continue;
        };
        next_connection += 1;
        tokio::spawn(serve(stream, next_connection, shared.clone(), permit));
    }
}

#[derive(Debug, thiserror::Error)]
enum HandshakeFailure {
    #[error("the connection closed during the handshake")]
    Closed,
    #[error("a bridge could not connect: {0}")]
    Protocol(String),
    #[error("a bridge proved a different pairing token; it re-reads the pairing file and retries")]
    Token,
    #[error("a bridge refused this app ({0:?}); its pairing file may be from an earlier start")]
    RefusedByBridge(Refusal),
    #[error("websocket: {0}")]
    Socket(#[from] tungstenite::Error),
}

impl HandshakeFailure {
    /// Failures that tell the user something about pairing or versions; a peer that simply
    /// went away does not.
    fn worth_reporting(&self) -> bool {
        matches!(
            self,
            HandshakeFailure::Protocol(_)
                | HandshakeFailure::Token
                | HandshakeFailure::RefusedByBridge(_)
        )
    }
}

async fn serve(stream: TcpStream, id: u64, shared: Arc<Shared>, permit: OwnedSemaphorePermit) {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    // The whole handshake, WebSocket upgrade included, must finish in time: a silent
    // connection is dropped.
    let deadline = time::Instant::now() + shared.timing.handshake;
    let Ok(Ok(mut socket)) =
        time::timeout_at(deadline, accept_async_with_config(stream, Some(config))).await
    else {
        return;
    };
    let session = match time::timeout_at(deadline, handshake(&mut socket, &shared.token)).await {
        Ok(Ok(session)) => session,
        Ok(Err(failure)) => {
            if failure.worth_reporting() {
                shared.report_problem(failure.to_string());
            }
            let _ = socket.close(None).await;
            return;
        }
        Err(_) => {
            let _ = socket.close(None).await;
            return;
        }
    };
    drop(permit);

    let (sender, mut outgoing) = mpsc::unbounded_channel();
    if shared.attach(session, Connection { id, sender }) == Attach::Superseded {
        refuse(&mut socket, Refusal::Superseded).await;
        return;
    }
    relay(&mut socket, id, &shared, &mut outgoing).await;
    shared.detach(id);
}

/// `hello` -> `welcome` -> `auth`: the session is trusted only after its proof verified.
async fn handshake(socket: &mut Socket, token: &str) -> Result<Session, HandshakeFailure> {
    let hello = next_text(socket).await?;
    let welcome_proof = match prove_hello(&hello, token) {
        Ok(welcome_proof) => welcome_proof,
        Err(message) => {
            refuse(socket, Refusal::Protocol).await;
            return Err(HandshakeFailure::Protocol(message));
        }
    };
    let own_nonce = nonce();
    send(
        socket,
        &AppMessage::Welcome {
            protocol: APP_PROTOCOL,
            nonce: own_nonce.clone(),
            proof: welcome_proof,
        },
    )
    .await?;

    let auth = next_text(socket).await?;
    match serde_json::from_str::<BridgeMessage>(&auth) {
        Ok(BridgeMessage::Auth { proof, session }) => {
            if !verify_proof(token, Prover::Bridge, &own_nonce, &proof) {
                refuse(socket, Refusal::Token).await;
                return Err(HandshakeFailure::Token);
            }
            Ok(session)
        }
        Ok(BridgeMessage::Refused { reason, .. }) => Err(HandshakeFailure::RefusedByBridge(reason)),
        Ok(_) => {
            refuse(socket, Refusal::Protocol).await;
            Err(HandshakeFailure::Protocol("expected auth".into()))
        }
        Err(error) => {
            refuse(socket, Refusal::Protocol).await;
            Err(HandshakeFailure::Protocol(format!(
                "unreadable auth: {error}"
            )))
        }
    }
}

/// Checks the `hello` and returns the app's proof over the bridge's nonce.
fn prove_hello(hello: &str, token: &str) -> Result<String, String> {
    check_protocol(hello).map_err(|error| error.to_string())?;
    match serde_json::from_str::<BridgeMessage>(hello) {
        Ok(BridgeMessage::Hello { nonce, .. }) => {
            proof(token, Prover::App, &nonce).map_err(|error| error.to_string())
        }
        Ok(_) => Err("expected hello".into()),
        Err(error) => Err(format!("unreadable hello: {error}")),
    }
}

async fn next_text(socket: &mut Socket) -> Result<String, HandshakeFailure> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.as_str().to_owned()),
            Some(Ok(Message::Binary(_))) => {
                return Err(HandshakeFailure::Protocol("binary message".into()));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Close(_))) | None => return Err(HandshakeFailure::Closed),
            Some(Err(error)) => return Err(error.into()),
        }
    }
}

async fn send(socket: &mut Socket, message: &AppMessage) -> Result<(), tungstenite::Error> {
    let text = serde_json::to_string(message).expect("app messages serialize");
    socket.send(Message::text(text)).await
}

async fn refuse(socket: &mut Socket, reason: Refusal) {
    let refused = AppMessage::Refused {
        protocol: APP_PROTOCOL,
        reason,
    };
    if send(socket, &refused).await.is_ok() {
        let _ = socket.close(None).await;
    }
}

/// Bridge messages go to the registry; the registry's messages go to the bridge. Ends when
/// either side closes or the bridge stops answering pings.
async fn relay(
    socket: &mut Socket,
    id: u64,
    shared: &Shared,
    outgoing: &mut mpsc::UnboundedReceiver<Outgoing>,
) {
    let ping_interval = shared.timing.ping_interval;
    let mut pings = time::interval_at(time::Instant::now() + ping_interval, ping_interval);
    let mut last_heard = time::Instant::now();
    loop {
        tokio::select! {
            incoming = socket.next() => {
                if let Some(Ok(_)) = &incoming {
                    last_heard = time::Instant::now();
                }
                match incoming {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<BridgeMessage>(&text) {
                        Ok(message) => shared.receive(id, message),
                        Err(error) => {
                            shared.report_problem(format!("unreadable message from a bridge: {error}"));
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
                }
            },
            _ = pings.tick() => {
                if last_heard.elapsed() >= ping_interval * MISSED_PONGS {
                    return;
                }
                if socket.send(Message::Ping(Default::default())).await.is_err() {
                    return;
                }
            },
            command = outgoing.recv() => match command {
                Some(Outgoing::Message(message)) => {
                    if send(socket, &message).await.is_err() {
                        return;
                    }
                }
                Some(Outgoing::Close) | None => {
                    let _ = socket.close(None).await;
                    return;
                }
            },
        }
    }
}
