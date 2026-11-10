//! An app host on a free port with temporary home and data directories, and a fake bridge
//! that speaks the real protocol types over a blocking WebSocket client.
#![allow(dead_code)]

use prism_app_host::registry::HostEvent;
use prism_app_host::{AppState, Host, HostConfig};
use prism_core::model::{AgentKind, Document, Element, ElementKind, Id, Question};
use prism_core::pairing::{PAIRING_FILE, Pairing};
use prism_core::protocol::*;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tungstenite::{Message, WebSocket};

pub const WAIT: Duration = Duration::from_secs(5);
pub const CONVERSATION: &str = "0b5b6c1e-4a5c-4c33-9a43-8f3a3b9c2d11";
pub const PROJECT: &str = "/workspace/project";

pub struct TestApp {
    pub host: Host,
    pub events: mpsc::Receiver<HostEvent>,
    pub home: TempDir,
    pub data: TempDir,
    // Dropped last: the host's tasks run on it.
    pub runtime: Runtime,
}

pub fn home_with(directories: &[&str]) -> TempDir {
    let home = TempDir::new().unwrap();
    for directory in directories {
        std::fs::create_dir(home.path().join(directory)).unwrap();
    }
    home
}

pub fn config(home: &Path, data: &Path) -> HostConfig {
    let mut config = HostConfig::new(home, data.to_path_buf());
    config.address = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config
}

pub fn start(home: TempDir, data: TempDir, adjust: impl FnOnce(&mut HostConfig)) -> TestApp {
    let runtime = Runtime::new().unwrap();
    let mut config = config(home.path(), data.path());
    adjust(&mut config);
    let (sender, events) = mpsc::channel();
    let sink = Box::new(move |event| {
        let _ = sender.send(event);
    });
    let host = runtime.block_on(Host::start(config, sink));
    TestApp {
        host,
        events,
        home,
        data,
        runtime,
    }
}

pub fn start_app() -> TestApp {
    start(home_with(&[".claude"]), TempDir::new().unwrap(), |_| {})
}

impl TestApp {
    pub fn port(&self) -> u16 {
        self.host.state().status.address.port()
    }

    pub fn pairing(&self) -> Pairing {
        let path = self.home.path().join(".claude").join(PAIRING_FILE);
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    pub fn token(&self) -> String {
        self.pairing().token
    }

    pub fn wait_for(&self, what: &str, condition: impl Fn(&AppState) -> bool) -> AppState {
        let deadline = Instant::now() + WAIT;
        loop {
            let state = self.host.state();
            if condition(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The next reply event, skipping other events.
    pub fn next_reply(&self) -> HostEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left).expect("a reply event") {
                event @ HostEvent::Reply { .. } => return event,
                _ => continue,
            }
        }
    }

    pub fn connect(&self, session: Session) -> FakeBridge {
        FakeBridge::authenticated(self.port(), &self.token(), session)
    }
}

pub fn key() -> String {
    format!("claude:{CONVERSATION}")
}

pub fn session(bound_at_ms: u64, binding: u128) -> Session {
    Session {
        host: "d_claude".into(),
        project: PROJECT.into(),
        agent: AgentKind::Claude,
        conversation: CONVERSATION.into(),
        cli: CliProcess {
            pid: 42,
            start_time: "123456".into(),
        },
        binding_id: Id::from_u128(binding),
        bound_at_ms,
    }
}

pub fn session_for(conversation: &str, project: &str) -> Session {
    Session {
        conversation: conversation.into(),
        project: project.into(),
        ..session(1, 1)
    }
}

pub fn document(id: u128, revision: u64, titles: &[&str]) -> Document {
    let elements = titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let element = Element {
                id: Id::from_u128(id * 100 + index as u128),
                title: (*title).into(),
                body: format!("{title} body"),
                kind: ElementKind::Step,
                source: None,
                order: index as u64,
                semantic_revision: revision,
                accepted_revision: None,
                archived: false,
            };
            (element.id, element)
        })
        .collect();
    Document {
        id: Id::from_u128(id),
        title: format!("Document {id}"),
        revision,
        elements,
        relationships: BTreeMap::new(),
        groups: BTreeMap::new(),
    }
}

pub struct FakeBridge {
    pub socket: WebSocket<TcpStream>,
}

impl FakeBridge {
    pub fn open(port: u16) -> Self {
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let (socket, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/"), stream).unwrap();
        Self { socket }
    }

    /// `hello`, verify the `welcome` proof, then `auth`.
    pub fn authenticated(port: u16, token: &str, session: Session) -> Self {
        let mut bridge = Self::open(port);
        let app_nonce = bridge.hello(token);
        let proof = proof(token, Prover::Bridge, &app_nonce).unwrap();
        bridge.send(&BridgeMessage::Auth { proof, session });
        bridge
    }

    /// A bridge that verifies the app but proves a different token in its `auth`.
    pub fn with_wrong_proof(port: u16, token: &str, session: Session) -> Self {
        let mut bridge = Self::open(port);
        let app_nonce = bridge.hello(token);
        let proof = proof("wrong token", Prover::Bridge, &app_nonce).unwrap();
        bridge.send(&BridgeMessage::Auth { proof, session });
        bridge
    }

    /// Sends `hello`, checks the app's proof and returns the app's nonce.
    pub fn hello(&mut self, token: &str) -> String {
        let own_nonce = nonce();
        self.send(&BridgeMessage::Hello {
            protocol: APP_PROTOCOL,
            nonce: own_nonce.clone(),
        });
        match self.receive() {
            AppMessage::Welcome {
                protocol,
                nonce,
                proof,
            } => {
                assert_eq!(protocol, APP_PROTOCOL);
                assert!(verify_proof(token, Prover::App, &own_nonce, &proof));
                nonce
            }
            other => panic!("expected welcome, got {other:?}"),
        }
    }

    pub fn send(&mut self, message: &BridgeMessage) {
        self.send_text(&serde_json::to_string(message).unwrap());
    }

    pub fn send_text(&mut self, text: &str) {
        self.socket.send(Message::text(text)).unwrap();
    }

    pub fn snapshot(&mut self, documents: Vec<Document>, questions: Vec<Question>) {
        self.send(&BridgeMessage::Snapshot {
            documents,
            questions,
            closed: Vec::new(),
            more: false,
        });
    }

    pub fn receive(&mut self) -> AppMessage {
        loop {
            match self.socket.read().expect("a message from the app") {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Close(_) => panic!("the app closed the connection"),
                _ => continue,
            }
        }
    }

    /// True once the app closed the connection; fails on any other message.
    pub fn closed(&mut self) -> bool {
        loop {
            match self.socket.read() {
                Ok(Message::Close(_)) => continue,
                Ok(Message::Text(text)) => panic!("expected close, got {text}"),
                Ok(_) => continue,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return true;
                }
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return false;
                }
                Err(_) => return true,
            }
        }
    }
}
