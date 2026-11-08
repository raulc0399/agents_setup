//! Native stdio MCP tools. Routing identity comes exclusively from Codex metadata.
use crate::bridge::{ActionParams, BridgeClient, InspectParams, Operation, QuestionParams};
use crate::model::{Id, MAX_PAYLOAD_BYTES};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::Path;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn id_schema() -> Value { json!({"type":"string","format":"uuid"}) }
fn optional_id() -> Value { json!({"anyOf":[id_schema(),{"type":"null"}]}) }

pub fn tools() -> Value {
    let element = object(json!({"id":id_schema(),"title":{"type":"string","maxLength":512},"body":{"type":"string","maxLength":65536},"kind":{"enum":["text","step","finding","question"]},"source":{"type":["string","null"]}}), &["id","title","body","kind","source"]);
    let relation = object(json!({"id":id_schema(),"from":id_schema(),"to":id_schema(),"label":{"type":"string","maxLength":512},"direction":{"enum":["directed","undirected","bidirectional"]}}), &["id","from","to","label","direction"]);
    let group = object(json!({"id":id_schema(),"title":{"type":"string","maxLength":512},"members":{"type":"array","items":id_schema(),"uniqueItems":true}}), &["id","title","members"]);
    json!({"tools":[
        {"name":"prism_publish","description":"Publish or revise requested visual content and open Prism beside this registered Codex conversation. Use fresh UUIDs only for new content; reuse returned IDs and expected revision for updates. Omitted records remain unchanged. Retrying the same request ID requires the identical payload.","inputSchema":object(json!({"request_id":id_schema(),"document_id":optional_id(),"expected_revision":{"type":["integer","null"],"minimum":0},"title":{"type":"string","maxLength":512},"elements":{"type":"array","items":element,"maxItems":2000},"relationships":{"type":"array","items":relation,"maxItems":4000},"groups":{"type":"array","items":group,"maxItems":500}}), &["request_id","document_id","expected_revision","title","elements","relationships","groups"])},
        {"name":"prism_inspect","description":"Inspect published Prism content in this exact conversation. Omit document_id to list document IDs and revisions. Private view selections and drafts are never returned.","inputSchema":object(json!({"document_id":optional_id()}), &[])},
        {"name":"prism_action","description":"Apply explicitly requested acceptance or archive to shared content IDs at the expected revision. Acceptance is content feedback, never permission to implement or run tools.","inputSchema":object(json!({"document_id":id_schema(),"expected_revision":{"type":"integer","minimum":0},"action":object(json!({"action":{"enum":["accept","archive"]},"ids":{"type":"array","items":id_schema(),"minItems":1,"maxItems":6500,"uniqueItems":true}}), &["action","ids"])}), &["document_id","expected_revision","action"])},
        {"name":"prism_questions","description":"Retrieve questions the user explicitly shared from Prism into this conversation. Omit request_id for summaries; supply it for the frozen selected-content snapshot. Answer in this CLI, then publish requested visual revisions. This never reads private drafts.","inputSchema":object(json!({"request_id":optional_id()}), &[])}
    ]})
}

pub fn native_thread(request: &Value) -> Result<String> {
    let meta = &request["params"]["_meta"];
    let thread = meta["threadId"].as_str().context("Codex native caller metadata is missing; do not supply identity in tool arguments")?;
    let id = Id::parse_str(thread).context("Codex thread ID is not a UUID")?;
    ensure!(!id.is_nil() && id.to_string() == thread, "Codex thread ID is not canonical");
    if let Some(nested) = meta["x-codex-turn-metadata"]["thread_id"].as_str() {
        ensure!(nested == thread, "inconsistent native caller metadata");
    }
    Ok(thread.to_owned())
}

pub fn operation(name: &str, arguments: Value) -> Result<Operation> {
    let fields: &[&str] = match name {
        "prism_publish" => &["request_id","document_id","expected_revision","title","elements","relationships","groups"],
        "prism_inspect" => &["document_id"],
        "prism_action" => &["document_id","expected_revision","action"],
        "prism_questions" => &["request_id"],
        _ => bail!("unknown Prism tool"),
    };
    let arguments = if arguments.is_null() { json!({}) } else { arguments };
    let object = arguments.as_object().context("tool arguments must be an object")?;
    ensure!(object.keys().all(|key| fields.contains(&key.as_str())), "unexpected tool argument; routing identity is never a tool argument");
    Ok(match name {
        "prism_publish" => Operation::Publish(serde_json::from_value(arguments)?),
        "prism_inspect" => Operation::Inspect(serde_json::from_value::<InspectParams>(arguments)?),
        "prism_action" => Operation::Action(serde_json::from_value::<ActionParams>(arguments)?),
        "prism_questions" => Operation::Questions(serde_json::from_value::<QuestionParams>(arguments)?),
        _ => unreachable!("tool name validated"),
    })
}

fn tool_result(request: &Value, binding_path: Option<&Path>) -> Result<Value> {
    let thread = native_thread(request)?;
    let binding_path = binding_path.with_context(|| format!("Prism requires a trusted tmux binding. Resume this exact conversation with: prism --agent codex --resume {thread}. An unregistered CLI cannot safely identify its pane."))?;
    let name = request["params"]["name"].as_str().context("tool name missing")?;
    let operation = operation(name, request["params"]["arguments"].clone())?;
    BridgeClient::new(binding_path)?.call(&thread, operation)
}

pub fn handle(request: &Value, binding_path: Option<&Path>) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => {
            let requested = request["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05");
            let supported = ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"];
            json!({"protocolVersion":if supported.contains(&requested) {requested} else {"2024-11-05"},"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"prism","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => tools(),
        "tools/call" => match tool_result(request, binding_path) {
            Ok(value) => json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}),
            Err(error) => json!({"content":[{"type":"text","text":error.to_string()}],"isError":true}),
        },
        _ => return Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub fn serve<R: BufRead, W: Write>(mut input: R, mut output: W, binding_path: Option<&Path>) -> Result<()> {
    loop {
        let mut bytes = Vec::new();
        let count = std::io::Read::take(&mut input, MAX_PAYLOAD_BYTES as u64 + 1).read_until(b'\n', &mut bytes)?;
        if count == 0 { return Ok(()); }
        ensure!(count <= MAX_PAYLOAD_BYTES, "MCP request exceeds payload limit");
        let response = match serde_json::from_slice::<Value>(&bytes) {
            Ok(request) => handle(&request, binding_path),
            Err(_) => Some(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"invalid JSON"}})),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}
