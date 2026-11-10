//! The bridge's link to the desktop app, against a fake app listening in this process.
use prism::actions::{ElementInput, PrepareQuestion, PublishRequest};
use prism::applink::{AppLink, LinkConfig, LinkState};
use prism::bridge::{
    ActionParams, AnswerParams, BridgeClient, LifecycleEvent, Operation, PendingParams,
    StatusParams, store_path,
};
use prism::model::{
    AgentKind, ContentAction, DeliveryMode, DeliveryOutcome, Document, ElementKind, Id, Question,
    Selection,
};
use prism::session::{LiveBinding, RuntimeDirectory, read_binding, write_binding};
use prism::store::Store;
use prism_core::pairing::Pairing;
use prism_core::protocol::{
    self, APP_PROTOCOL, AppMessage, BridgeMessage, CommandError, MAX_MESSAGE_BYTES, Prover,
    Refusal, Session,
};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_POLL: Duration = Duration::from_millis(5);
/// Far longer than the test backoff: a link that still retries connects within it.
const QUIET: Duration = Duration::from_millis(400);
/// A tool operation that waited for the link would take at least its handshake timeout.
const FAST: Duration = Duration::from_secs(2);
/// The bridge waits at most two seconds for its link at the end of the session.
const PROMPT_EXIT: Duration = Duration::from_secs(4);

type AppSocket = WebSocket<TcpStream>;
type BridgeThread = JoinHandle<anyhow::Result<()>>;

struct Fixture {
    project: TempDir,
    /// The attached Claude CLI.
    cli: Child,
    binding_path: PathBuf,
    binding: LiveBinding,
    conversation: String,
    app: TcpListener,
    token: String,
    pairing: PathBuf,
}

impl Fixture {
    /// A Claude binding with its conversation bound, and a fake app with its pairing file.
    fn new() -> Self {
        let mut fixture = Self::unbound();
        let conversation = fixture.conversation.clone();
        fixture.binding.bind_conversation(&conversation).unwrap();
        write_binding(&fixture.binding_path, &fixture.binding).unwrap();
        fixture
    }

    /// A binding whose conversation is not bound yet.
    fn unbound() -> Self {
        let project = TempDir::new().unwrap();
        let runtime = RuntimeDirectory::create(project.path()).unwrap();
        let cli = Command::new("sleep").arg("60").spawn().unwrap();
        let binding = LiveBinding::new(
            project.path(),
            AgentKind::Claude,
            format!("claude-cli://{}", cli.id()),
            cli.id(),
        )
        .unwrap();
        runtime.write_binding(&binding).unwrap();
        let app = TcpListener::bind("127.0.0.1:0").unwrap();
        app.set_nonblocking(true).unwrap();
        let token = Uuid::new_v4().simple().to_string();
        let pairing = project.path().join("app.json");
        write_pairing(&pairing, app.local_addr().unwrap().port(), &token);
        Self {
            project,
            cli,
            binding_path: runtime.binding_path(),
            binding,
            conversation: Uuid::new_v4().to_string(),
            app,
            token,
            pairing,
        }
    }

    fn config(&self) -> LinkConfig {
        LinkConfig {
            pairing_file: Some(self.pairing.clone()),
            host: "127.0.0.1".into(),
            retry_initial: Duration::from_millis(10),
            retry_max: Duration::from_millis(40),
            poll_interval: Duration::from_millis(10),
        }
    }

    fn start_bridge(&self) -> BridgeThread {
        let path = self.binding_path.clone();
        let config = self.config();
        let bridge = std::thread::spawn(move || prism::bridge::run_with_link(&path, config));
        let deadline = Instant::now() + WAIT;
        while !prism::bridge::is_ready(&self.binding_path) {
            assert!(Instant::now() < deadline, "bridge did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        bridge
    }

    fn call(&self, operation: Operation) -> serde_json::Value {
        BridgeClient::new(&self.binding_path)
            .unwrap()
            .call(&self.conversation, operation)
            .unwrap()
    }

    /// Publishes one element through the agent tool path; returns the document and element.
    fn publish(&self, title: &str) -> (Id, Id) {
        let element = Id::new_v4();
        let result = self.call(Operation::Publish(PublishRequest {
            request_id: Id::new_v4(),
            document_id: None,
            expected_revision: None,
            title: title.into(),
            elements: vec![ElementInput {
                id: element,
                title: "Node".into(),
                body: "Body".into(),
                kind: ElementKind::Text,
                source: None,
            }],
            relationships: vec![],
            groups: vec![],
        }));
        let document = Id::parse_str(result["data"]["document_id"].as_str().unwrap()).unwrap();
        (document, element)
    }

    /// Ends the session the way the Claude mod reports `/clear` while the app is connected:
    /// the bridge sends `closed` and exits with the retirement reason.
    fn stop(&self, bridge: BridgeThread, mut socket: AppSocket) {
        self.end_session();
        let BridgeMessage::Closed { reason } = recv(&mut socket) else {
            panic!("expected closed");
        };
        assert!(reason.contains("clear"), "{reason}");
        drop(socket);
        let error = bridge.join().unwrap().unwrap_err().to_string();
        assert!(error.contains("clear"), "{error}");
    }

    fn end_session(&self) {
        let result = self.call(Operation::Lifecycle(LifecycleEvent::SessionEnd {
            reason: "clear".into(),
        }));
        assert_eq!(result["data"]["retiring"], true);
    }

    fn accept(&self) -> AppSocket {
        tungstenite::accept(self.accept_raw()).unwrap()
    }

    /// The bridge's next TCP connection, before any WebSocket handshake.
    fn accept_raw(&self) -> TcpStream {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.app.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
                    return stream;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "the bridge did not connect");
                    std::thread::sleep(ACCEPT_POLL);
                }
                Err(error) => panic!("{error}"),
            }
        }
    }

    fn connects_within(&self, period: Duration) -> bool {
        let deadline = Instant::now() + period;
        while Instant::now() < deadline {
            if self.app.accept().is_ok() {
                return true;
            }
            std::thread::sleep(ACCEPT_POLL);
        }
        false
    }

    /// The app's side of a full handshake: checks the bridge's proof, returns its session and
    /// the snapshot.
    fn handshake(&self, socket: &mut AppSocket) -> (Session, Vec<Document>, Vec<Question>) {
        let app_nonce = welcome(socket, &self.token, APP_PROTOCOL);
        let BridgeMessage::Auth { proof, session } = recv(socket) else {
            panic!("expected auth");
        };
        assert!(protocol::verify_proof(
            &self.token,
            Prover::Bridge,
            &app_nonce,
            &proof
        ));
        let (documents, questions) = snapshot(socket);
        (session, documents, questions)
    }

    fn connect(&self) -> (AppSocket, Session, Vec<Document>, Vec<Question>) {
        let mut socket = self.accept();
        let (session, documents, questions) = self.handshake(&mut socket);
        (socket, session, documents, questions)
    }

    /// Publishes straight into the store, as another process would; returns the document and
    /// its first element.
    fn store_publish(&self, title: &str, elements: usize, body: &str) -> (Id, Id) {
        let (mut store, workspace) = self.store();
        let elements = (0..elements)
            .map(|index| ElementInput {
                id: Id::new_v4(),
                title: format!("Node {index}"),
                body: body.into(),
                kind: ElementKind::Text,
                source: None,
            })
            .collect::<Vec<_>>();
        let first = elements[0].id;
        let result = store
            .publish(
                workspace,
                &PublishRequest {
                    request_id: Id::new_v4(),
                    document_id: None,
                    expected_revision: None,
                    title: title.into(),
                    elements,
                    relationships: vec![],
                    groups: vec![],
                },
            )
            .unwrap();
        (result.document_id, first)
    }

    fn store(&self) -> (Store, Id) {
        let binding = read_binding(&self.binding_path).unwrap();
        let mut store = Store::open(store_path(&binding)).unwrap();
        let workspace = store.workspace(&binding.workspace_key().unwrap()).unwrap();
        (store, workspace.id)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.cli.kill();
        let _ = self.cli.wait();
    }
}

fn write_pairing(path: &Path, port: u16, token: &str) {
    let pairing = Pairing {
        port,
        token: token.into(),
    };
    std::fs::write(path, serde_json::to_vec(&pairing).unwrap()).unwrap();
}

fn recv(socket: &mut AppSocket) -> BridgeMessage {
    loop {
        match socket.read().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Close(_) => panic!("the bridge closed the connection"),
            _ => {}
        }
    }
}

fn send(socket: &mut AppSocket, message: &AppMessage) {
    send_text(socket, &serde_json::to_string(message).unwrap());
}

fn send_text(socket: &mut AppSocket, text: &str) {
    socket.send(Message::Text(text.into())).unwrap();
}

/// A loopback port that refuses connections for as long as the returned socket lives: bound
/// but never listening, so no other listener can take it meanwhile.
fn refusing_port() -> (OwnedFd, u16) {
    // SAFETY: plain socket calls on a descriptor owned here, with correctly sized addresses.
    unsafe {
        let descriptor = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        assert!(descriptor >= 0);
        let socket = OwnedFd::from_raw_fd(descriptor);
        let mut address: libc::sockaddr_in = std::mem::zeroed();
        address.sin_family = libc::AF_INET as libc::sa_family_t;
        address.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
        let mut length = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        assert_eq!(
            libc::bind(
                descriptor,
                (&address as *const libc::sockaddr_in).cast(),
                length
            ),
            0
        );
        assert_eq!(
            libc::getsockname(
                descriptor,
                (&mut address as *mut libc::sockaddr_in).cast(),
                &mut length
            ),
            0
        );
        (socket, u16::from_be(address.sin_port))
    }
}

/// Reads `hello` and answers `welcome` proven with `token`; returns the app's nonce.
fn welcome(socket: &mut AppSocket, token: &str, protocol_number: u32) -> String {
    let BridgeMessage::Hello { protocol, nonce } = recv(socket) else {
        panic!("expected hello");
    };
    assert_eq!(protocol, APP_PROTOCOL);
    let app_nonce = protocol::nonce();
    send(
        socket,
        &AppMessage::Welcome {
            protocol: protocol_number,
            nonce: app_nonce.clone(),
            proof: protocol::proof(token, Prover::App, &nonce).unwrap(),
        },
    );
    app_nonce
}

fn snapshot(socket: &mut AppSocket) -> (Vec<Document>, Vec<Question>) {
    let (documents, questions, _) = snapshot_with_closed(socket);
    (documents, questions)
}

/// The whole snapshot with the closed documents of all its frames.
fn snapshot_with_closed(socket: &mut AppSocket) -> (Vec<Document>, Vec<Question>, Vec<Id>) {
    let mut documents = Vec::new();
    let mut questions = Vec::new();
    let mut closed = Vec::new();
    loop {
        let BridgeMessage::Snapshot {
            documents: frame_documents,
            questions: frame_questions,
            closed: frame_closed,
            more,
        } = recv(socket)
        else {
            panic!("expected a snapshot frame");
        };
        documents.extend(frame_documents);
        questions.extend(frame_questions);
        closed.extend(frame_closed);
        if !more {
            return (documents, questions, closed);
        }
    }
}

fn update(socket: &mut AppSocket) -> (Vec<Document>, Vec<Question>) {
    let (documents, questions, _) = update_with_closed(socket);
    (documents, questions)
}

fn update_with_closed(socket: &mut AppSocket) -> (Vec<Document>, Vec<Question>, Vec<Id>) {
    let BridgeMessage::Update {
        documents,
        questions,
        closed,
    } = recv(socket)
    else {
        panic!("expected update");
    };
    (documents, questions, closed)
}

fn reply(socket: &mut AppSocket, id: Id) -> Option<CommandError> {
    let BridgeMessage::Reply { id: replied, error } = recv(socket) else {
        panic!("expected reply");
    };
    assert_eq!(replied, id);
    error
}

fn refused(socket: &mut AppSocket) -> Refusal {
    let BridgeMessage::Refused { protocol, reason } = recv(socket) else {
        panic!("expected refused");
    };
    assert_eq!(protocol, APP_PROTOCOL);
    reason
}

/// After refusing, the bridge closes without sending anything else.
fn expect_end(socket: &mut AppSocket) {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => panic!("the bridge sent {text} after refusing"),
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

#[test]
fn handshake_announces_the_session_sends_the_snapshot_and_closed() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, _) = fixture.publish("Before the app");
    let (socket, session, documents, questions) = fixture.connect();
    let binding = read_binding(&fixture.binding_path).unwrap();
    assert_eq!(
        Path::new(&session.project),
        fixture.project.path().canonicalize().unwrap()
    );
    assert_eq!(session.agent, AgentKind::Claude);
    assert_eq!(session.conversation, fixture.conversation);
    assert_eq!(session.binding_id, binding.binding_id);
    assert_eq!(session.cli.pid, binding.server_pid);
    assert_eq!(session.cli.start_time, binding.server_start_time);
    assert_ne!(session.bound_at_ms, 0);
    assert_eq!(session.bound_at_ms, binding.bound_at_ms);
    assert!(!session.host.is_empty());
    assert_eq!(
        documents
            .iter()
            .map(|document| document.id)
            .collect::<Vec<_>>(),
        vec![document]
    );
    assert!(questions.is_empty());
    fixture.stop(bridge, socket);
}

#[test]
fn bad_welcome_proof_is_refused_and_the_pairing_file_is_read_again() {
    let fixture = Fixture::new();
    // The pairing file holds another token than the app proves with.
    let port = fixture.app.local_addr().unwrap().port();
    write_pairing(&fixture.pairing, port, "stale-token");
    let bridge = fixture.start_bridge();
    let mut socket = fixture.accept();
    welcome(&mut socket, &fixture.token, APP_PROTOCOL);
    assert_eq!(refused(&mut socket), Refusal::Token);
    expect_end(&mut socket);
    // The app wrote a new file; the next attempt reads it and authenticates.
    write_pairing(&fixture.pairing, port, &fixture.token);
    let deadline = Instant::now() + WAIT;
    let socket = loop {
        let mut socket = fixture.accept();
        let app_nonce = welcome(&mut socket, &fixture.token, APP_PROTOCOL);
        match recv(&mut socket) {
            BridgeMessage::Auth { proof, .. } => {
                assert!(protocol::verify_proof(
                    &fixture.token,
                    Prover::Bridge,
                    &app_nonce,
                    &proof
                ));
                snapshot(&mut socket);
                break socket;
            }
            BridgeMessage::Refused { reason, .. } => assert_eq!(reason, Refusal::Token),
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "the new pairing file was not read"
        );
    };
    fixture.stop(bridge, socket);
}

#[test]
fn token_refusal_from_the_app_is_retried() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (mut socket, ..) = fixture.connect();
    send(
        &mut socket,
        &AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Token,
        },
    );
    drop(socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

#[test]
fn different_protocol_is_refused_and_retried() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let mut socket = fixture.accept();
    welcome(&mut socket, &fixture.token, APP_PROTOCOL + 1);
    assert_eq!(refused(&mut socket), Refusal::Protocol);
    expect_end(&mut socket);
    // An app that refuses `hello` for its newer protocol gets no `refused` back.
    let mut socket = fixture.accept();
    let BridgeMessage::Hello { .. } = recv(&mut socket) else {
        panic!("expected hello");
    };
    send(
        &mut socket,
        &AppMessage::Refused {
            protocol: APP_PROTOCOL + 1,
            reason: Refusal::Protocol,
        },
    );
    expect_end(&mut socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

/// Waits until the link's last error contains `expected` and returns it.
fn link_error(link: &AppLink, expected: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = link.status();
        if let Some(error) = status.error.filter(|error| error.contains(expected)) {
            assert_eq!(status.state, LinkState::Retrying);
            return error;
        }
        assert!(Instant::now() < deadline, "no error containing {expected}");
        std::thread::sleep(ACCEPT_POLL);
    }
}

#[test]
fn a_protocol_mismatch_names_both_numbers_and_the_older_side() {
    let fixture = Fixture::new();
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    let newer = APP_PROTOCOL + 1;
    let older = APP_PROTOCOL - 1;

    // A newer app refuses the hello.
    let mut socket = fixture.accept();
    let BridgeMessage::Hello { .. } = recv(&mut socket) else {
        panic!("expected hello");
    };
    send(
        &mut socket,
        &AppMessage::Refused {
            protocol: newer,
            reason: Refusal::Protocol,
        },
    );
    expect_end(&mut socket);
    let error = link_error(&link, "this bridge is older");
    assert_eq!(
        error,
        format!(
            "the Prism app speaks protocol {newer}, this bridge protocol {APP_PROTOCOL}; this \
             bridge is older: install the newer prism binary in this container, then restart \
             the conversation"
        )
    );

    // An older app's welcome is refused.
    let mut socket = fixture.accept();
    welcome(&mut socket, &fixture.token, older);
    assert_eq!(refused(&mut socket), Refusal::Protocol);
    expect_end(&mut socket);
    let error = link_error(&link, "the app is older");
    assert_eq!(
        error,
        format!(
            "the Prism app speaks protocol {older}, this bridge protocol {APP_PROTOCOL}; the app \
             is older: build and install the newer Prism app on the host"
        )
    );

    // The message stays while the link retries; a connection clears it.
    let (socket, ..) = fixture.connect();
    let deadline = Instant::now() + WAIT;
    while link.status().state != LinkState::Connected {
        assert!(Instant::now() < deadline, "the link did not connect");
        std::thread::sleep(ACCEPT_POLL);
    }
    assert!(link.status().error.is_none());
    close(link, socket);
}

#[test]
fn malformed_app_nonce_is_refused_and_retried() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let mut socket = fixture.accept();
    let BridgeMessage::Hello { nonce, .. } = recv(&mut socket) else {
        panic!("expected hello");
    };
    // The app's proof verifies, but the bridge cannot prove over its nonce.
    send(
        &mut socket,
        &AppMessage::Welcome {
            protocol: APP_PROTOCOL,
            nonce: "not-a-nonce".into(),
            proof: protocol::proof(&fixture.token, Prover::App, &nonce).unwrap(),
        },
    );
    assert_eq!(refused(&mut socket), Refusal::Protocol);
    expect_end(&mut socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

#[test]
fn oversized_app_message_ends_only_that_connection() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (mut socket, ..) = fixture.connect();
    // The bridge may drop the connection while this frame is still being written.
    let _ = socket.send(Message::Text("x".repeat(MAX_MESSAGE_BYTES + 1).into()));
    expect_end(&mut socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

#[test]
fn malformed_command_gets_an_invalid_reply_and_a_message_without_id_closes() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (mut socket, ..) = fixture.connect();
    let id = Id::new_v4();
    send_text(
        &mut socket,
        &serde_json::json!({"type":"action","id":id,"document_id":"not-an-id"}).to_string(),
    );
    assert!(matches!(
        reply(&mut socket, id),
        Some(CommandError::Invalid { .. })
    ));
    // The connection stays: the next command is served on it.
    let fetch = Id::new_v4();
    send(&mut socket, &AppMessage::Fetch { id: fetch });
    snapshot(&mut socket);
    assert_eq!(reply(&mut socket, fetch), None);
    send_text(&mut socket, r#"{"type":"unknown"}"#);
    expect_end(&mut socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

#[test]
fn reconnect_after_the_app_drops_replays_the_snapshot() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (socket, first, documents, _) = fixture.connect();
    assert!(documents.is_empty());
    drop(socket);
    let (document, _) = fixture.publish("While the app was away");
    let (socket, second, documents, _) = fixture.connect();
    assert_eq!(
        documents
            .iter()
            .map(|document| document.id)
            .collect::<Vec<_>>(),
        vec![document]
    );
    // The same binding reconnecting: the app replaces the connection instead of refusing it.
    assert_eq!(first, second);
    fixture.stop(bridge, socket);
}

#[test]
fn commits_from_the_tools_and_other_processes_arrive_as_updates() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (mut socket, ..) = fixture.connect();
    let (document, element) = fixture.publish("Published");
    let (documents, questions) = update(&mut socket);
    assert_eq!(documents.len(), 1);
    assert_eq!((documents[0].id, documents[0].revision), (document, 1));
    assert!(questions.is_empty());
    // The terminal UI is a separate process with its own connection to the store.
    let (mut store, workspace) = fixture.store();
    store
        .apply(
            workspace,
            document,
            1,
            &ContentAction::Accept { ids: vec![element] },
        )
        .unwrap();
    let (documents, _) = update(&mut socket);
    assert_eq!((documents[0].id, documents[0].revision), (document, 2));
    fixture.stop(bridge, socket);
}

#[test]
fn commands_apply_through_the_shared_actions() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, element) = fixture.publish("Commands");
    let (mut socket, ..) = fixture.connect();

    // Ask: the command ID becomes the request ID of a question shared for the prompt box.
    let ask = Id::new_v4();
    send(
        &mut socket,
        &AppMessage::Ask {
            id: ask,
            selection: Selection {
                document_id: document,
                revision: 1,
                ids: vec![element],
            },
            text: "Item 1: Node\n> Body".into(),
        },
    );
    let (_, questions) = update(&mut socket);
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].request_id, ask);
    assert_eq!(questions[0].mode, DeliveryMode::Share);
    assert_eq!(questions[0].outcome, DeliveryOutcome::Pending);
    assert_eq!(reply(&mut socket, ask), None);
    let pending = fixture.call(Operation::Pending(PendingParams {}));
    assert_eq!(
        pending["data"]["questions"][0]["request_id"],
        ask.to_string()
    );

    // Action, then its resend against the old revision.
    let archive = |id| AppMessage::Action {
        id,
        document_id: document,
        expected_revision: 1,
        action: ContentAction::Archive { ids: vec![element] },
    };
    let first = Id::new_v4();
    send(&mut socket, &archive(first));
    let (documents, _) = update(&mut socket);
    assert_eq!(documents[0].revision, 2);
    assert!(documents[0].elements[&element].archived);
    assert_eq!(reply(&mut socket, first), None);
    let resent = Id::new_v4();
    send(&mut socket, &archive(resent));
    assert_eq!(
        reply(&mut socket, resent),
        Some(CommandError::StaleRevision { current: 2 })
    );

    // Undo, its resend, and an undo with nothing left.
    let undo = |id, expected_revision| AppMessage::Undo {
        id,
        document_id: document,
        expected_revision,
    };
    let first = Id::new_v4();
    send(&mut socket, &undo(first, 2));
    let (documents, _) = update(&mut socket);
    assert_eq!(documents[0].revision, 3);
    assert!(!documents[0].elements[&element].archived);
    assert_eq!(reply(&mut socket, first), None);
    let resent = Id::new_v4();
    send(&mut socket, &undo(resent, 2));
    assert_eq!(
        reply(&mut socket, resent),
        Some(CommandError::StaleRevision { current: 3 })
    );
    let empty = Id::new_v4();
    send(&mut socket, &undo(empty, 3));
    assert!(matches!(
        reply(&mut socket, empty),
        Some(CommandError::NotFound { .. })
    ));

    // Fetch answers with the whole content, then the reply.
    let fetch = Id::new_v4();
    send(&mut socket, &AppMessage::Fetch { id: fetch });
    let (documents, questions) = snapshot(&mut socket);
    assert_eq!((documents[0].id, documents[0].revision), (document, 3));
    assert_eq!(questions[0].request_id, ask);
    assert_eq!(reply(&mut socket, fetch), None);
    fixture.stop(bridge, socket);
}

#[test]
fn closing_from_the_app_or_the_tools_crosses_as_the_closed_set() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, element) = fixture.publish("Closing");
    let (mut socket, ..) = fixture.connect();
    let action = |id, expected_revision, action| AppMessage::Action {
        id,
        document_id: document,
        expected_revision,
        action,
    };

    // Close from the app: the document stays in the content and the closed set names it.
    let close = Id::new_v4();
    send(&mut socket, &action(close, 1, ContentAction::Close {}));
    let (documents, _, closed) = update_with_closed(&mut socket);
    assert_eq!((documents[0].id, documents[0].revision), (document, 2));
    assert_eq!(closed, vec![document]);
    assert_eq!(reply(&mut socket, close), None);
    let resent = Id::new_v4();
    send(&mut socket, &action(resent, 1, ContentAction::Close {}));
    assert_eq!(
        reply(&mut socket, resent),
        Some(CommandError::StaleRevision { current: 2 })
    );

    // A snapshot carries it too.
    let fetch = Id::new_v4();
    send(&mut socket, &AppMessage::Fetch { id: fetch });
    let (documents, _, closed) = snapshot_with_closed(&mut socket);
    assert_eq!(documents.len(), 1);
    assert_eq!(closed, vec![document]);
    assert_eq!(reply(&mut socket, fetch), None);

    // Undo right after the close reopens it.
    let undo = Id::new_v4();
    send(
        &mut socket,
        &AppMessage::Undo {
            id: undo,
            document_id: document,
            expected_revision: 2,
        },
    );
    let (documents, _, closed) = update_with_closed(&mut socket);
    assert_eq!(documents[0].revision, 3);
    assert!(closed.is_empty());
    assert_eq!(reply(&mut socket, undo), None);

    // The agent closes it through its tool, then revises it, which reopens it.
    fixture.call(Operation::Action(ActionParams {
        document_id: document,
        expected_revision: 3,
        action: ContentAction::Close {},
    }));
    let (_, _, closed) = update_with_closed(&mut socket);
    assert_eq!(closed, vec![document]);
    fixture.call(Operation::Publish(PublishRequest {
        request_id: Id::new_v4(),
        document_id: Some(document),
        expected_revision: Some(4),
        title: "Closing, revised".into(),
        elements: vec![ElementInput {
            id: element,
            title: "Node".into(),
            body: "Body".into(),
            kind: ElementKind::Text,
            source: None,
        }],
        relationships: vec![],
        groups: vec![],
    }));
    let (documents, _, closed) = update_with_closed(&mut socket);
    assert_eq!(documents[0].title, "Closing, revised");
    assert!(closed.is_empty());
    fixture.stop(bridge, socket);
}

#[test]
fn ask_resent_after_reconnect_is_shared_once() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, element) = fixture.publish("Resend");
    let (mut socket, ..) = fixture.connect();
    let id = Id::new_v4();
    let ask = AppMessage::Ask {
        id,
        selection: Selection {
            document_id: document,
            revision: 1,
            ids: vec![element],
        },
        text: "Item 1: Node".into(),
    };
    send(&mut socket, &ask);
    drop(socket);
    // The ask committed, but its reply never reached the app.
    let (store, workspace) = fixture.store();
    let deadline = Instant::now() + WAIT;
    while store.question(workspace, id).is_err() {
        assert!(Instant::now() < deadline, "the ask did not commit");
        std::thread::sleep(ACCEPT_POLL);
    }
    let (mut socket, _, _, questions) = fixture.connect();
    assert_eq!(questions.len(), 1);
    send(&mut socket, &ask);
    // The store returns the question it holds: no update, a plain reply.
    assert_eq!(reply(&mut socket, id), None);
    assert_eq!(store.questions(workspace).unwrap().len(), 1);
    let pending = fixture.call(Operation::Pending(PendingParams {}));
    assert_eq!(pending["data"]["questions"].as_array().unwrap().len(), 1);
    fixture.stop(bridge, socket);
}

#[test]
fn superseded_after_a_verified_welcome_stops_retrying() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (mut socket, ..) = fixture.connect();
    send(
        &mut socket,
        &AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded,
        },
    );
    drop(socket);
    assert!(!fixture.connects_within(QUIET));
    // The tools keep working for the superseded binding.
    fixture.publish("After superseded");
    fixture.end_session();
    assert!(bridge.join().unwrap().is_err());
}

#[test]
fn superseded_before_a_verified_welcome_is_retried() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let mut socket = fixture.accept();
    let BridgeMessage::Hello { .. } = recv(&mut socket) else {
        panic!("expected hello");
    };
    send(
        &mut socket,
        &AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded,
        },
    );
    drop(socket);
    let (socket, ..) = fixture.connect();
    fixture.stop(bridge, socket);
}

#[test]
fn absent_or_silent_app_leaves_tool_operations_working_and_fast() {
    let fixture = Fixture::new();
    std::fs::remove_file(&fixture.pairing).unwrap();
    let bridge = fixture.start_bridge();
    let timed_publish = |title| {
        let started = Instant::now();
        fixture.publish(title);
        let elapsed = started.elapsed();
        assert!(elapsed < FAST, "{title} took {elapsed:?}");
    };
    timed_publish("No pairing file");
    // A pairing file whose port refuses connections.
    let (_refusing, refusing) = refusing_port();
    write_pairing(&fixture.pairing, refusing, &fixture.token);
    std::thread::sleep(QUIET);
    timed_publish("Nothing listening");
    // An app that accepts the connection but never answers holds the link in its handshake;
    // the tools must not wait for it, and neither must the end of the session.
    let port = fixture.app.local_addr().unwrap().port();
    write_pairing(&fixture.pairing, port, &fixture.token);
    let _silent = fixture.accept_raw();
    timed_publish("Silent app");
    let started = Instant::now();
    fixture.call(Operation::Pending(PendingParams {}));
    assert!(started.elapsed() < FAST);
    fixture.end_session();
    let started = Instant::now();
    assert!(bridge.join().unwrap().is_err());
    let elapsed = started.elapsed();
    assert!(elapsed < PROMPT_EXIT, "bridge exit took {elapsed:?}");
}

#[test]
fn questions_not_shared_never_cross() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, element) = fixture.publish("Private questions");
    let (mut store, workspace) = fixture.store();
    let private = |text: &str| PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: document,
            revision: 1,
            ids: vec![element],
        },
        text: text.into(),
        mode: DeliveryMode::Start,
    };
    store
        .prepare_question(workspace, &private("before the app"))
        .unwrap();
    let (mut socket, _, _, questions) = fixture.connect();
    assert!(questions.is_empty());
    store
        .prepare_question(workspace, &private("while connected"))
        .unwrap();
    let (later, _) = fixture.publish("Later");
    let (documents, questions) = update(&mut socket);
    assert_eq!(
        documents
            .iter()
            .map(|document| document.id)
            .collect::<Vec<_>>(),
        vec![later]
    );
    assert!(questions.is_empty());
    fixture.stop(bridge, socket);
}

#[test]
fn answer_and_prompt_box_reports_arrive_as_updates() {
    let fixture = Fixture::new();
    let bridge = fixture.start_bridge();
    let (document, element) = fixture.publish("Answered");
    let (mut socket, ..) = fixture.connect();
    let ask = Id::new_v4();
    send(
        &mut socket,
        &AppMessage::Ask {
            id: ask,
            selection: Selection {
                document_id: document,
                revision: 1,
                ids: vec![element],
            },
            text: "Item 1: Node".into(),
        },
    );
    update(&mut socket);
    assert_eq!(reply(&mut socket, ask), None);
    // The mod places the question in the prompt box.
    fixture.call(Operation::Lifecycle(LifecycleEvent::PromptFilled {
        request_id: ask,
    }));
    let (_, questions) = update(&mut socket);
    assert_eq!(questions[0].outcome, DeliveryOutcome::Submitted);
    // The agent records its answer.
    fixture.call(Operation::Answer(AnswerParams {
        request_id: ask,
        text: "Because.".into(),
    }));
    let (_, questions) = update(&mut socket);
    assert_eq!(questions[0].answer.as_deref(), Some("Because."));
    fixture.stop(bridge, socket);
}

#[test]
fn commands_are_refused_for_an_unverified_or_retired_binding() {
    let mut fixture = Fixture::new();
    let (document, element) = fixture.store_publish("Refused", 1, "Body");
    let (store, workspace) = fixture.store();
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    let (mut socket, ..) = fixture.connect();
    // The bound CLI process exited.
    fixture.cli.kill().unwrap();
    fixture.cli.wait().unwrap();
    let ask = Id::new_v4();
    send(
        &mut socket,
        &AppMessage::Ask {
            id: ask,
            selection: Selection {
                document_id: document,
                revision: 1,
                ids: vec![element],
            },
            text: "Item 1: Node 0".into(),
        },
    );
    assert!(matches!(
        reply(&mut socket, ask),
        Some(CommandError::Failed { .. })
    ));
    let mut binding = read_binding(&fixture.binding_path).unwrap();
    binding.retire().unwrap();
    write_binding(&fixture.binding_path, &binding).unwrap();
    let archive = Id::new_v4();
    send(
        &mut socket,
        &AppMessage::Action {
            id: archive,
            document_id: document,
            expected_revision: 1,
            action: ContentAction::Archive { ids: vec![element] },
        },
    );
    let Some(CommandError::Failed { message }) = reply(&mut socket, archive) else {
        panic!("expected a failed command");
    };
    assert!(message.contains("retired"), "{message}");
    assert_eq!(store.document(workspace, document).unwrap().revision, 1);
    assert!(store.questions(workspace).unwrap().is_empty());
    close(link, socket);
}

#[test]
fn large_snapshot_is_sent_in_frames() {
    let fixture = Fixture::new();
    let body = "x".repeat(prism::model::MAX_TEXT_BYTES - 1024);
    for index in 0..3 {
        fixture.store_publish(&format!("Large {index}"), 20, &body);
    }
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    let mut socket = fixture.accept();
    let app_nonce = welcome(&mut socket, &fixture.token, APP_PROTOCOL);
    let BridgeMessage::Auth { proof, .. } = recv(&mut socket) else {
        panic!("expected auth");
    };
    assert!(protocol::verify_proof(
        &fixture.token,
        Prover::Bridge,
        &app_nonce,
        &proof
    ));
    let mut frames = 0;
    let mut documents = 0;
    loop {
        let BridgeMessage::Snapshot {
            documents: frame,
            more,
            ..
        } = recv(&mut socket)
        else {
            panic!("expected a snapshot frame");
        };
        frames += 1;
        documents += frame.len();
        if !more {
            break;
        }
    }
    assert!(frames > 1, "{frames} frame");
    assert_eq!(documents, 3);
    close(link, socket);
}

#[test]
fn link_waits_for_the_conversation_and_keeps_the_bound_time_across_restarts() {
    let mut fixture = Fixture::unbound();
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    assert!(!fixture.connects_within(QUIET));
    // The first tool call binds the conversation; an older binding records no bound time.
    let conversation = fixture.conversation.clone();
    fixture.binding.bind_conversation(&conversation).unwrap();
    fixture.binding.bound_at_ms = 0;
    write_binding(&fixture.binding_path, &fixture.binding).unwrap();
    let (socket, first, ..) = fixture.connect();
    assert_eq!(first.conversation, fixture.conversation);
    // Never 0, and older than any recorded bind time.
    assert_eq!(first.bound_at_ms, 1);
    assert_eq!(
        read_binding(&fixture.binding_path).unwrap().bound_at_ms,
        first.bound_at_ms
    );
    close(link, socket);
    // A restarted bridge reports the same bound time.
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    let (socket, second, ..) = fixture.connect();
    assert_eq!(second, first);
    close(link, socket);
}

#[test]
fn status_reports_the_app_link_and_its_pairing_file() {
    let fixture = Fixture::new();
    let missing = fixture.project.path().join("missing-app.json");
    let config = LinkConfig {
        pairing_file: Some(missing.clone()),
        ..fixture.config()
    };
    let link = AppLink::start(&fixture.binding_path, config).unwrap();
    let deadline = Instant::now() + WAIT;
    while link.status().state != LinkState::Retrying {
        assert!(
            Instant::now() < deadline,
            "the link did not report its retry"
        );
        std::thread::sleep(ACCEPT_POLL);
    }
    let status = link.status();
    assert_eq!(status.pairing_file, Some(missing));
    assert!(!status.pairing_found);
    assert_eq!(status.port, None);
    assert_eq!(status.host, "127.0.0.1");
    assert!(status.error.unwrap().contains("no pairing file"));
    link.close("bridge stopped");

    // The bridge serves the same status to the mod's `/prism` through `status`.
    let bridge = fixture.start_bridge();
    let (socket, ..) = fixture.connect();
    let deadline = Instant::now() + WAIT;
    let app = loop {
        let app = fixture.call(Operation::Status(StatusParams {}))["data"]["app"].clone();
        if app["state"] == "connected" {
            break app;
        }
        assert!(
            Instant::now() < deadline,
            "the link did not report its connection"
        );
        std::thread::sleep(ACCEPT_POLL);
    };
    assert_eq!(app["port"], fixture.app.local_addr().unwrap().port());
    assert_eq!(app["pairing_found"], true);
    assert_eq!(app["pairing_file"], fixture.pairing.to_str().unwrap());
    assert!(app["since_ms"].as_u64().unwrap() > 0);
    assert!(app["error"].is_null());
    fixture.stop(bridge, socket);
}

#[test]
fn status_is_connecting_during_the_first_attempt_and_waiting_while_unbound() {
    let fixture = Fixture::new();
    let link = AppLink::start(&fixture.binding_path, fixture.config()).unwrap();
    // The app takes the TCP connection but never answers the WebSocket handshake.
    let stalled = fixture.accept_raw();
    let status = link.status();
    assert_eq!(status.state, LinkState::Connecting);
    assert_eq!(status.port, Some(fixture.app.local_addr().unwrap().port()));
    assert!(status.pairing_found);
    assert!(status.error.is_none());
    drop(stalled);
    link.close("bridge stopped");

    let unbound = Fixture::unbound();
    let link = AppLink::start(&unbound.binding_path, unbound.config()).unwrap();
    let deadline = Instant::now() + WAIT;
    while link.status().state != LinkState::Waiting {
        assert!(
            Instant::now() < deadline,
            "the link did not report the unbound binding"
        );
        std::thread::sleep(ACCEPT_POLL);
    }
    assert!(!unbound.connects_within(QUIET));
    link.close("bridge stopped");
}

/// Closes a link while the app is connected: the app hears `closed` with the reason.
fn close(link: AppLink, mut socket: AppSocket) {
    let closing = std::thread::spawn(move || link.close("bridge stopped"));
    let BridgeMessage::Closed { reason } = recv(&mut socket) else {
        panic!("expected closed");
    };
    assert_eq!(reason, "bridge stopped");
    drop(socket);
    closing.join().unwrap();
}
