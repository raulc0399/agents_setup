use prism::mcp::{handle, native_thread, operation, serve, tools};
use serde_json::{Value, json};
use std::io::Cursor;
use uuid::Uuid;

#[test]
fn oversized_line_is_drained_before_next_request() {
    for newline in [true, false] {
        let mut input = vec![b'x'; prism::model::MAX_PAYLOAD_BYTES * 2];
        if newline {
            input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n");
        }
        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output, None).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(replies[0]["error"].is_object());
        assert_eq!(replies.len(), if newline { 2 } else { 1 });
        if newline {
            assert_eq!(replies[1]["id"], 3);
        }
    }
}

#[test]
fn native_identity_rejects_malformed_nested_metadata() {
    let thread = Uuid::new_v4().to_string();
    for nested in [
        json!({"thread_id": 123}),
        json!("invalid"),
        json!({"thread_id": Uuid::new_v4()}),
    ] {
        assert!(
            native_thread(
                &json!({"params":{"_meta":{"threadId":thread,"x-codex-turn-metadata":nested}}})
            )
            .is_err()
        );
    }
}

#[test]
fn native_identity_uses_only_canonical_metadata() {
    let thread = Uuid::new_v4().to_string();
    assert_eq!(native_thread(&json!({"params":{"_meta":{"threadId":thread,"x-codex-turn-metadata":{"thread_id":thread}}}})).unwrap(), thread);
    assert!(native_thread(&json!({"params":{"arguments":{"threadId":thread}}})).is_err());
    assert!(native_thread(&json!({"params":{"_meta":{"threadId":Uuid::nil()}}})).is_err());
    assert!(operation("prism_inspect", json!({"thread_id":thread})).is_err());
}

#[test]
fn invalid_json_rpc_envelope_is_rejected() {
    for request in [
        json!({"id":1,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":{},"method":"ping"}),
        json!([]),
    ] {
        assert_eq!(handle(&request, None).unwrap()["error"]["code"], -32600);
    }
}

#[test]
fn protocol_negotiation_and_notifications_are_compatible() {
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let response = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version}}), None).unwrap();
        assert_eq!(response["result"]["protocolVersion"], version);
    }
    assert!(
        handle(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            None
        )
        .is_none()
    );
    assert_eq!(tools()["tools"].as_array().unwrap().len(), 5);
    for name in ["prism_inspect", "prism_questions"] {
        assert!(operation(name, json!({})).is_ok());
    }
}

#[test]
fn missing_native_thread_cannot_attach() {
    let result = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"prism_inspect","arguments":{}}}), None).unwrap();
    assert_eq!(result["result"]["isError"], true);
    assert!(
        result["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("native caller metadata is missing")
    );
}

#[test]
fn stdio_recovers_from_bad_json_without_replying_to_notifications() {
    let input = b"invalid\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n";
    let mut output = Vec::new();
    serve(Cursor::new(input), &mut output, None).unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["result"], json!({}));
}
