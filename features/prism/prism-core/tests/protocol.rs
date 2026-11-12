use prism_core::model::*;
use prism_core::protocol::*;
use serde_json::json;
use std::collections::BTreeMap;

const DOCUMENT: u128 = 1;
const ELEMENT: u128 = 2;
const QUESTION: u128 = 3;
const WORKSPACE: u128 = 4;
const BINDING: u128 = 5;
const COMMAND: u128 = 6;

fn document() -> Document {
    let element = Element {
        id: Id::from_u128(ELEMENT),
        title: "Collect".into(),
        body: "Reads the device".into(),
        kind: ElementKind::Step,
        source: Some("src/device.rs".into()),
        order: 0,
        created_revision: None,
        semantic_revision: 1,
        accepted_revision: None,
        archived: false,
    };
    Document {
        id: Id::from_u128(DOCUMENT),
        title: "Flow".into(),
        revision: 1,
        elements: BTreeMap::from([(element.id, element)]),
        relationships: BTreeMap::new(),
        groups: BTreeMap::new(),
        lanes: None,
        bookmarks: BTreeMap::new(),
    }
}

fn selection() -> Selection {
    Selection {
        document_id: Id::from_u128(DOCUMENT),
        revision: 1,
        ids: vec![Id::from_u128(ELEMENT)],
    }
}

fn question() -> Question {
    let document = document();
    Question {
        request_id: Id::from_u128(QUESTION),
        workspace_id: Id::from_u128(WORKSPACE),
        generation: 1,
        snapshot: SelectionSnapshot {
            selection: selection(),
            document_title: document.title,
            elements: document.elements,
            relationships: BTreeMap::new(),
            groups: BTreeMap::new(),
        },
        text: "Item 1: Collect\n> Reads the device".into(),
        mode: DeliveryMode::Share,
        outcome: DeliveryOutcome::Pending,
        answer: Some("It polls.".into()),
        intent: None,
        quoted: None,
    }
}

fn session(bound_at_ms: u64, binding: u128) -> Session {
    Session {
        host: "d_claude".into(),
        project: "/workspace/project".into(),
        agent: AgentKind::Claude,
        conversation: "0b5b6c1e-4a5c-4c33-9a43-8f3a3b9c2d11".into(),
        cli: CliProcess {
            pid: 42,
            start_time: "123456".into(),
        },
        binding_id: Id::from_u128(binding),
        bound_at_ms,
    }
}

fn hello() -> BridgeMessage {
    BridgeMessage::Hello {
        protocol: APP_PROTOCOL,
        nonce: "bridge-nonce".into(),
    }
}

fn auth() -> BridgeMessage {
    BridgeMessage::Auth {
        proof: "00ff".into(),
        session: session(1_000, BINDING),
    }
}

fn round_trip<T>(message: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let text = serde_json::to_string(message).unwrap();
    assert_eq!(
        &serde_json::from_str::<T>(&text).unwrap(),
        message,
        "{text}"
    );
}

/// The literal is the wire format: the message writes it and reads back from it.
fn fixed<T>(message: &T, literal: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        serde_json::from_str::<serde_json::Value>(literal).unwrap()
    );
    assert_eq!(&serde_json::from_str::<T>(literal).unwrap(), message);
}

#[test]
fn bridge_messages_round_trip() {
    for message in [
        hello(),
        auth(),
        BridgeMessage::Snapshot {
            documents: vec![document()],
            questions: vec![question()],
            closed: vec![Id::from_u128(DOCUMENT)],
            more: true,
        },
        BridgeMessage::Update {
            documents: vec![document()],
            questions: vec![question()],
            closed: Vec::new(),
        },
        BridgeMessage::Closed {
            reason: "session end".into(),
        },
        BridgeMessage::Reply {
            id: Id::from_u128(COMMAND),
            error: None,
        },
        BridgeMessage::Reply {
            id: Id::from_u128(COMMAND),
            error: Some(CommandError::UndoConflict),
        },
        BridgeMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Token,
        },
    ] {
        round_trip(&message);
    }
}

#[test]
fn app_messages_round_trip() {
    let id = Id::from_u128(COMMAND);
    for message in [
        AppMessage::Welcome {
            protocol: APP_PROTOCOL,
            nonce: "app-nonce".into(),
            proof: "cd".repeat(32),
        },
        AppMessage::Ask {
            id,
            selection: selection(),
            text: "Item 1: Collect\n> Reads the device".into(),
            intent: None,
            quoted: None,
        },
        AppMessage::Ask {
            id,
            selection: selection(),
            text: "Item 1: Collect\n> Reads the device\nExplain".into(),
            intent: Some(Intent::Explain),
            quoted: None,
        },
        AppMessage::Action {
            id,
            document_id: Id::from_u128(DOCUMENT),
            expected_revision: 1,
            action: ContentAction::Archive {
                ids: vec![Id::from_u128(ELEMENT)],
            },
        },
        AppMessage::Undo {
            id,
            document_id: Id::from_u128(DOCUMENT),
            expected_revision: 2,
        },
        AppMessage::Fetch { id },
        AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded,
        },
    ] {
        round_trip(&message);
    }
}

#[test]
fn hello_has_a_fixed_wire_format() {
    fixed(
        &hello(),
        r#"{"type":"hello","protocol":3,"nonce":"bridge-nonce"}"#,
    );
}

#[test]
fn auth_has_a_fixed_wire_format() {
    fixed(
        &auth(),
        r#"{"type":"auth","proof":"00ff","session":{"host":"d_claude","project":"/workspace/project","agent":"claude","conversation":"0b5b6c1e-4a5c-4c33-9a43-8f3a3b9c2d11","cli":{"pid":42,"start_time":"123456"},"binding_id":"00000000-0000-0000-0000-000000000005","bound_at_ms":1000}}"#,
    );
}

#[test]
fn welcome_has_a_fixed_wire_format() {
    fixed(
        &AppMessage::Welcome {
            protocol: APP_PROTOCOL,
            nonce: "app-nonce".into(),
            proof: "00ff".into(),
        },
        r#"{"type":"welcome","protocol":3,"nonce":"app-nonce","proof":"00ff"}"#,
    );
}

#[test]
fn refused_has_a_fixed_wire_format_in_both_directions() {
    let literal = r#"{"type":"refused","protocol":3,"reason":"superseded"}"#;
    fixed(
        &AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded,
        },
        literal,
    );
    fixed(
        &BridgeMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded,
        },
        literal,
    );
}

#[test]
fn reply_has_a_fixed_wire_format() {
    fixed(
        &BridgeMessage::Reply {
            id: Id::from_u128(COMMAND),
            error: None,
        },
        r#"{"type":"reply","id":"00000000-0000-0000-0000-000000000006","error":null}"#,
    );
    fixed(
        &BridgeMessage::Reply {
            id: Id::from_u128(COMMAND),
            error: Some(CommandError::StaleRevision { current: 7 }),
        },
        r#"{"type":"reply","id":"00000000-0000-0000-0000-000000000006","error":{"code":"stale_revision","current":7}}"#,
    );
    assert_eq!(
        CommandError::StaleRevision { current: 7 }.to_string(),
        "revision conflict; current revision is 7"
    );
}

#[test]
fn snapshot_has_a_fixed_wire_format() {
    fixed(
        &BridgeMessage::Snapshot {
            documents: Vec::new(),
            questions: Vec::new(),
            closed: vec![Id::from_u128(DOCUMENT)],
            more: false,
        },
        r#"{"type":"snapshot","documents":[],"questions":[],"closed":["00000000-0000-0000-0000-000000000001"],"more":false}"#,
    );
}

#[test]
fn update_has_a_fixed_wire_format_with_the_whole_closed_set() {
    fixed(
        &BridgeMessage::Update {
            documents: Vec::new(),
            questions: Vec::new(),
            closed: vec![Id::from_u128(DOCUMENT)],
        },
        r#"{"type":"update","documents":[],"questions":[],"closed":["00000000-0000-0000-0000-000000000001"]}"#,
    );
}

#[test]
fn action_carries_the_content_action_and_expected_revision() {
    fixed(
        &AppMessage::Action {
            id: Id::from_u128(COMMAND),
            document_id: Id::from_u128(DOCUMENT),
            expected_revision: 3,
            action: ContentAction::Accept {
                ids: vec![Id::from_u128(ELEMENT)],
            },
        },
        r#"{"type":"action","id":"00000000-0000-0000-0000-000000000006","document_id":"00000000-0000-0000-0000-000000000001","expected_revision":3,"action":{"action":"accept","ids":["00000000-0000-0000-0000-000000000002"]}}"#,
    );
    fixed(
        &AppMessage::Action {
            id: Id::from_u128(COMMAND),
            document_id: Id::from_u128(DOCUMENT),
            expected_revision: 3,
            action: ContentAction::Close {},
        },
        r#"{"type":"action","id":"00000000-0000-0000-0000-000000000006","document_id":"00000000-0000-0000-0000-000000000001","expected_revision":3,"action":{"action":"close"}}"#,
    );
    fixed(&ContentAction::Reopen {}, r#"{"action":"reopen"}"#);
    // A document action takes no item IDs.
    let error = serde_json::from_str::<ContentAction>(
        r#"{"action":"close","ids":["00000000-0000-0000-0000-000000000002"]}"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field `ids`"), "{error}");
}

/// Bookmark actions, which protocol 3 added: an item mark leaves `quote` out.
#[test]
fn bookmark_actions_have_a_fixed_wire_format() {
    fixed(
        &ContentAction::Bookmark {
            ids: vec![Id::from_u128(ELEMENT)],
            quote: None,
        },
        r#"{"action":"bookmark","ids":["00000000-0000-0000-0000-000000000002"]}"#,
    );
    fixed(
        &ContentAction::Bookmark {
            ids: vec![Id::from_u128(ELEMENT)],
            quote: Some("the device".into()),
        },
        r#"{"action":"bookmark","ids":["00000000-0000-0000-0000-000000000002"],"quote":"the device"}"#,
    );
    fixed(
        &ContentAction::Unbookmark {
            ids: vec![Id::from_u128(ELEMENT)],
        },
        r#"{"action":"unbookmark","ids":["00000000-0000-0000-0000-000000000002"]}"#,
    );
    let error = serde_json::from_str::<ContentAction>(
        r#"{"action":"unbookmark","ids":["00000000-0000-0000-0000-000000000002"],"quote":"x"}"#,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("unknown field `quote`"),
        "{error}"
    );
}

/// Bookmarks cross with their document; a document without them serializes as before, and
/// one stored before them loads without them.
#[test]
fn bookmarks_are_optional_records_of_the_document() {
    let plain = document();
    assert!(
        serde_json::to_value(&plain)
            .unwrap()
            .get("bookmarks")
            .is_none()
    );
    let mut older = serde_json::to_value(&plain).unwrap();
    older.as_object_mut().unwrap().remove("bookmarks");
    assert_eq!(serde_json::from_value::<Document>(older).unwrap(), plain);

    let mut marked = document();
    let bookmark = Bookmark {
        id: Id::from_u128(9),
        item: Id::from_u128(ELEMENT),
        quote: Some("the device".into()),
        order: 4,
    };
    marked.bookmarks.insert(bookmark.id, bookmark);
    let value = serde_json::to_value(&marked).unwrap();
    assert_eq!(
        value["bookmarks"],
        json!({"00000000-0000-0000-0000-000000000009":{"id":"00000000-0000-0000-0000-000000000009","item":"00000000-0000-0000-0000-000000000002","quote":"the device","order":4}})
    );
    assert_eq!(serde_json::from_value::<Document>(value).unwrap(), marked);
}

/// The intent and the quoted item are optional fields: an ordinary ask leaves them out, so its
/// wire format is the one before explain requests, and an older peer ignores them.
#[test]
fn ask_carries_an_intent_only_for_an_explain_request() {
    let ask = |text: &str, intent, quoted| AppMessage::Ask {
        id: Id::from_u128(COMMAND),
        selection: selection(),
        text: text.into(),
        intent,
        quoted,
    };
    let selection = r#""selection":{"document_id":"00000000-0000-0000-0000-000000000001","revision":1,"ids":["00000000-0000-0000-0000-000000000002"]}"#;
    fixed(
        &ask("Item 1: Collect", None, None),
        &format!(
            r#"{{"type":"ask","id":"00000000-0000-0000-0000-000000000006",{selection},"text":"Item 1: Collect"}}"#
        ),
    );
    fixed(
        &ask(
            "Item 1: Collect\n> Reads\nExplain",
            Some(Intent::Explain),
            Some(Id::from_u128(ELEMENT)),
        ),
        &format!(
            r#"{{"type":"ask","id":"00000000-0000-0000-0000-000000000006",{selection},"text":"Item 1: Collect\n> Reads\nExplain","intent":"explain","quoted":"00000000-0000-0000-0000-000000000002"}}"#
        ),
    );
}

/// Questions stored and sent before explain requests have no intent and still load.
#[test]
fn a_question_without_intent_loads_and_serializes_without_one() {
    let mut value = serde_json::to_value(question()).unwrap();
    assert!(value.get("intent").is_none());
    value.as_object_mut().unwrap().remove("answer");
    let loaded = serde_json::from_value::<Question>(value).unwrap();
    assert_eq!(
        (loaded.intent, loaded.quoted, loaded.answer),
        (None, None, None)
    );
    let explain = Question {
        intent: Some(Intent::Explain),
        ..question()
    };
    assert_eq!(serde_json::to_value(&explain).unwrap()["intent"], "explain");
    round_trip(&explain);
}

#[test]
fn unknown_fields_are_ignored_and_unknown_types_refused() {
    let mut value = serde_json::to_value(auth()).unwrap();
    value["compression"] = json!("zstd");
    value["session"]["container"] = json!("abc");
    assert_eq!(
        serde_json::from_value::<BridgeMessage>(value).unwrap(),
        auth()
    );
    assert!(serde_json::from_value::<AppMessage>(json!({"type": "delete_all"})).is_err());
}

#[test]
fn a_newer_protocol_is_detected_before_the_message_parses() {
    let current = serde_json::to_string(&hello()).unwrap();
    assert_eq!(check_protocol(&current), Ok(()));

    let mut newer = serde_json::to_value(hello()).unwrap();
    newer["protocol"] = json!(APP_PROTOCOL + 1);
    newer["nonce"] = json!({"reshaped": true});
    let newer = newer.to_string();
    assert!(serde_json::from_str::<BridgeMessage>(&newer).is_err());
    assert_eq!(
        check_protocol(&newer),
        Err(ProtocolError::Unsupported {
            theirs: APP_PROTOCOL + 1
        })
    );

    let refused = serde_json::to_string(&AppMessage::Refused {
        protocol: APP_PROTOCOL + 1,
        reason: Refusal::Protocol,
    })
    .unwrap();
    assert!(check_protocol(&refused).is_err());
    assert_eq!(
        check_protocol(r#"{"type":"fetch"}"#),
        Err(ProtocolError::Missing)
    );
    assert_eq!(check_protocol("not json"), Err(ProtocolError::Missing));
}

#[test]
fn the_later_binding_supersedes_an_earlier_one_for_the_same_conversation() {
    let earlier = session(1_000, BINDING);
    let later = session(2_000, BINDING - 1);
    assert!(later.supersedes(&earlier));
    assert!(!earlier.supersedes(&later));
    assert!(!earlier.supersedes(&earlier));

    let tie = session(1_000, BINDING + 1);
    assert!(tie.supersedes(&earlier));

    let other = Session {
        conversation: "another".into(),
        ..later
    };
    assert!(!other.supersedes(&earlier));
}

#[test]
fn proofs_verify_only_with_the_right_token_role_and_challenge() {
    let token = "t".repeat(64);
    let bridge_nonce = nonce();
    let app_nonce = nonce();
    assert_ne!(bridge_nonce, app_nonce);

    let app_proof = proof(&token, Prover::App, &bridge_nonce).unwrap();
    assert!(verify_proof(&token, Prover::App, &bridge_nonce, &app_proof));
    let wrong_token = "u".repeat(64);
    assert!(!verify_proof(
        &wrong_token,
        Prover::App,
        &bridge_nonce,
        &app_proof
    ));
    assert!(!verify_proof(
        &token,
        Prover::Bridge,
        &bridge_nonce,
        &app_proof
    ));
    assert!(!verify_proof(&token, Prover::App, &app_nonce, &app_proof));
    assert!(!verify_proof(&token, Prover::App, &bridge_nonce, "zz"));
    assert!(!verify_proof(
        &token,
        Prover::App,
        &bridge_nonce,
        &app_proof[..62]
    ));
    let empty_token_proof = proof("", Prover::App, &bridge_nonce).unwrap();
    assert!(!verify_proof(
        "",
        Prover::App,
        &bridge_nonce,
        &empty_token_proof
    ));

    let bridge_proof = proof(&token, Prover::Bridge, &app_nonce).unwrap();
    assert!(verify_proof(
        &token,
        Prover::Bridge,
        &app_nonce,
        &bridge_proof
    ));
}

#[test]
fn only_64_lowercase_hex_nonces_are_proven_over_or_verified() {
    let token = "t".repeat(64);
    let valid = nonce();
    assert_eq!(valid.len(), 64);
    assert!(proof(&token, Prover::App, &valid).is_ok());

    let upper = valid.to_uppercase();
    let not_hex = format!("{}g", &valid[..63]);
    let multibyte = format!("{}é", &valid[..62]);
    for invalid in ["", &valid[..63], &upper, &not_hex, &multibyte] {
        assert_eq!(
            proof(&token, Prover::App, invalid),
            Err(ProtocolError::InvalidNonce),
            "{invalid}"
        );
        let proven = proof(&token, Prover::App, &valid).unwrap();
        assert!(!verify_proof(&token, Prover::App, invalid, &proven));
    }
}

#[test]
fn proofs_are_hmac_sha256_over_the_label_and_the_challenge() {
    let challenge = "0123456789abcdef".repeat(4);
    assert_eq!(
        proof("Jefe", Prover::App, &challenge).unwrap(),
        "53656000093644b41df6e8c1975b984f12f0dff5c10bdc7c9cc4cb75c3e3bd21"
    );
    assert_eq!(
        proof("Jefe", Prover::Bridge, &challenge).unwrap(),
        "3d9beaecec21158e0a368156db3baf73c51b98f43e6d58692af09e08a3ede3a2"
    );
}

#[test]
fn snapshot_frames_stay_under_the_message_limit_and_end_with_the_last_one() {
    let large = |id: u128| {
        let mut document = document();
        document.id = Id::from_u128(id);
        for element in document.elements.values_mut() {
            element.body = "x".repeat(1_200_000);
        }
        document
    };
    let documents = vec![large(101), large(102), large(103)];
    let closed = vec![Id::from_u128(102)];
    let frames = snapshot_frames(documents.clone(), vec![question()], closed.clone()).unwrap();
    assert_eq!(frames.len(), 3);

    let mut sent_documents = Vec::new();
    let mut sent_questions = Vec::new();
    let mut sent_closed = Vec::new();
    let mut markers = Vec::new();
    for frame in frames {
        assert!(serde_json::to_vec(&frame).unwrap().len() <= MAX_MESSAGE_BYTES);
        let BridgeMessage::Snapshot {
            documents,
            questions,
            closed,
            more,
        } = frame
        else {
            panic!("not a snapshot frame");
        };
        sent_documents.extend(documents);
        sent_questions.extend(questions);
        sent_closed.push(closed);
        markers.push(more);
    }
    assert_eq!(markers, vec![true, true, false]);
    assert_eq!(sent_documents, documents);
    assert_eq!(sent_questions, vec![question()]);
    // The closed set goes once, in the first frame.
    assert_eq!(sent_closed, vec![closed, Vec::new(), Vec::new()]);

    assert_eq!(
        snapshot_frames(Vec::new(), Vec::new(), Vec::new()).unwrap(),
        vec![BridgeMessage::Snapshot {
            documents: Vec::new(),
            questions: Vec::new(),
            closed: Vec::new(),
            more: false,
        }]
    );
}

#[test]
fn a_record_too_large_for_one_frame_is_an_error() {
    let mut document = document();
    for element in document.elements.values_mut() {
        element.body = "x".repeat(MAX_PAYLOAD_BYTES);
    }
    let error = snapshot_frames(vec![document], Vec::new(), Vec::new()).unwrap_err();
    assert!(
        matches!(error, FrameError::TooLarge { bytes } if bytes > MAX_PAYLOAD_BYTES),
        "{error}"
    );
}
