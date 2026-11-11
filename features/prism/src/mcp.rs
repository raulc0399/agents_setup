//! Native stdio MCP tools. Routing identity comes exclusively from Codex metadata.
use crate::bridge::{
    ActionParams, AnswerParams, BridgeClient, InspectParams, Operation, QuestionParams,
};
use crate::model::{Id, MAX_PAYLOAD_BYTES};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

// Tool text nests a bounded bridge JSON response inside another JSON string.
const MAX_RESPONSE_BYTES: usize = 8 * MAX_PAYLOAD_BYTES + 64 * 1024;

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn id_schema() -> Value {
    json!({"type":"string","format":"uuid"})
}
fn optional_id() -> Value {
    json!({"anyOf":[id_schema(),{"type":"null"}]})
}

pub fn tools() -> Value {
    let element = object(
        json!({"id":id_schema(),"title":{"type":"string","maxLength":512},"body":{"type":"string","maxLength":65536},"kind":{"enum":["text","step","finding","question"]},"source":{"type":["string","null"]}}),
        &["id", "title", "body", "kind", "source"],
    );
    let relation = object(
        json!({"id":id_schema(),"from":id_schema(),"to":id_schema(),"label":{"type":"string","maxLength":512},"direction":{"enum":["directed","undirected","bidirectional"]},"manual":{"type":"boolean","description":"A manual or review step; diagrams draw it dashed."}}),
        &["id", "from", "to", "label", "direction"],
    );
    let group = object(
        json!({"id":id_schema(),"title":{"type":"string","maxLength":512},"members":{"type":"array","items":id_schema(),"uniqueItems":true}}),
        &["id", "title", "members"],
    );
    json!({"tools":[
        {"name":"prism_publish","description":"Publish or revise requested visual content in Prism for this Codex conversation; the Prism app shows it. Use fresh UUIDs only for new content; reuse returned IDs and expected revision for updates. Omitted records remain unchanged. Every element, relationship and group keeps the position of its first publication: what a publish adds goes after the last record, and nothing existing moves. Diagrams number arrows in that order and never renumber them, so list relationships in the order the data flows; when a flow changes in the middle, publish the diagram as a new document instead of adding a mid-flow arrow. For a diagram keep to at most 7 boxes, 9 arrows and 3 lanes per document; lanes are laid out as rows or columns automatically. Retrying the same request ID requires the identical payload.","inputSchema":object(json!({"request_id":id_schema(),"document_id":optional_id(),"expected_revision":{"type":["integer","null"],"minimum":0},"title":{"type":"string","maxLength":512},"elements":{"type":"array","items":element,"maxItems":2000},"relationships":{"type":"array","items":relation,"maxItems":4000},"groups":{"type":"array","items":group,"maxItems":500},"lanes":{"anyOf":[{"enum":["auto","rows","columns"]},{"type":"null"}],"description":"How a diagram lays out its lanes (groups): as rows stacked top to bottom, or as columns side by side. Prism chooses automatically at the first publish with two lanes and keeps that choice; omitted or null keeps the current one, \"auto\" chooses again. Set rows or columns only when the automatic choice reads badly."}}), &["request_id","document_id","expected_revision","title","elements","relationships","groups"])},
        {"name":"prism_inspect","description":"Inspect published Prism content in this exact conversation. Omit document_id to list at most 100 document summaries, each with its closed flag; repeat with returned next_cursor until null. Private view state such as selections, hidden items and filters is never returned.","inputSchema":object(json!({"document_id":optional_id(),"cursor":optional_id()}), &[])},
        {"name":"prism_action","description":"Apply explicitly requested acceptance or archive to shared content IDs, or close or reopen the whole document (without ids), at the expected revision. A closed document leaves the document strip for everyone and stays listed under Closed until it is reopened or published to again. Acceptance is content feedback, never permission to implement or run tools.","inputSchema":object(json!({"document_id":id_schema(),"expected_revision":{"type":"integer","minimum":0},"action":object(json!({"action":{"enum":["accept","archive","close","reopen"]},"ids":{"type":"array","items":id_schema(),"minItems":1,"maxItems":6500,"uniqueItems":true,"description":"Required for accept and archive; omitted for close and reopen."}}), &["action"])}), &["document_id","expected_revision","action"])},
        {"name":"prism_questions","description":"Retrieve questions the user explicitly shared from Prism into this conversation. Omit request_id for at most 100 summaries with a 512-character text preview and text_truncated flag; repeat with returned next_cursor until null; supply it for the frozen selected-content snapshot. Answer in this CLI, then publish requested visual revisions. Questions not shared for retrieval are never returned.","inputSchema":object(json!({"request_id":optional_id(),"cursor":optional_id()}), &[])},
        {"name":"prism_answer","description":"Not used for now; do not call it. Answers are kept in the conversation and saved to the md files when relevant; they are not recorded in Prism, which changes only when the item itself changes and you publish the revised item. The tool records a reply to a Prism question beside the item; a later call replaces the recorded text.","inputSchema":object(json!({"request_id":id_schema(),"text":{"type":"string","maxLength":65536}}), &["request_id","text"])}
    ]})
}

pub fn native_thread(request: &Value) -> Result<String> {
    let meta = &request["params"]["_meta"];
    let thread = meta["threadId"].as_str().context(
        "Codex native caller metadata is missing; do not supply identity in tool arguments",
    )?;
    let id = Id::parse_str(thread).context("Codex thread ID is not a UUID")?;
    ensure!(
        !id.is_nil() && id.to_string() == thread,
        "Codex thread ID is not canonical"
    );
    if let Some(nested) = meta.get("x-codex-turn-metadata") {
        let nested_thread = nested
            .get("thread_id")
            .and_then(Value::as_str)
            .context("malformed native turn metadata")?;
        ensure!(
            nested_thread == thread,
            "inconsistent native caller metadata"
        );
    }
    Ok(thread.to_owned())
}

pub fn operation(name: &str, arguments: Value) -> Result<Operation> {
    let fields: &[&str] = match name {
        "prism_publish" => &[
            "request_id",
            "document_id",
            "expected_revision",
            "title",
            "elements",
            "relationships",
            "groups",
            "lanes",
        ],
        "prism_inspect" => &["document_id", "cursor"],
        "prism_action" => &["document_id", "expected_revision", "action"],
        "prism_questions" => &["request_id", "cursor"],
        "prism_answer" => &["request_id", "text"],
        _ => bail!("unknown Prism tool"),
    };
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    let object = arguments
        .as_object()
        .context("tool arguments must be an object")?;
    ensure!(
        object.keys().all(|key| fields.contains(&key.as_str())),
        "unexpected tool argument; routing identity is never a tool argument"
    );
    Ok(match name {
        "prism_publish" => Operation::Publish(serde_json::from_value(arguments)?),
        "prism_inspect" => Operation::Inspect(serde_json::from_value::<InspectParams>(arguments)?),
        "prism_action" => Operation::Action(serde_json::from_value::<ActionParams>(arguments)?),
        "prism_questions" => {
            Operation::Questions(serde_json::from_value::<QuestionParams>(arguments)?)
        }
        "prism_answer" => Operation::Answer(serde_json::from_value::<AnswerParams>(arguments)?),
        _ => unreachable!("tool name validated"),
    })
}

fn tool_result(request: &Value) -> Result<Value> {
    let thread = native_thread(request)?;
    let name = request["params"]["name"]
        .as_str()
        .context("tool name missing")?;
    let operation = operation(name, request["params"]["arguments"].clone())?;

    let binding_path = crate::attachment::ensure_attached(
        crate::model::AgentKind::Codex,
        &thread,
        std::process::id(),
    )?;
    let binding = crate::session::read_binding(&binding_path)?;
    ensure!(
        binding.agent == crate::model::AgentKind::Codex,
        "Prism MCP requires a Codex binding"
    );
    ensure!(
        binding
            .conversation
            .as_deref()
            .is_none_or(|bound| bound == thread),
        "Only the bound Codex thread may use this Prism binding; child and unrelated threads must not reuse it"
    );
    BridgeClient::new(&binding_path)?.call(&thread, operation)
}

pub fn handle(request: &Value) -> Option<Value> {
    let valid_id = request
        .get("id")
        .is_none_or(|id| id.is_string() || id.is_number() || id.is_null());
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || !request.get("method").is_some_and(Value::is_string)
        || !valid_id
        || request
            .get("params")
            .is_some_and(|params| !params.is_object())
    {
        return Some(
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"invalid request"}}),
        );
    }
    let id = request.get("id")?.clone();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => {
            let requested = request["params"]["protocolVersion"]
                .as_str()
                .unwrap_or("2024-11-05");
            let supported = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
            json!({"protocolVersion":if supported.contains(&requested) {requested} else {"2024-11-05"},"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"prism","version":env!("CARGO_PKG_VERSION")}})
        }
        "ping" => json!({}),
        "tools/list" => tools(),
        "tools/call" => match tool_result(request) {
            Ok(value) => {
                json!({"content":[{"type":"text","text":value.to_string()}],"isError":false})
            }
            Err(error) => {
                json!({"content":[{"type":"text","text":error.to_string()}],"isError":true})
            }
        },
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}),
            );
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub fn serve<R: BufRead, W: Write>(mut input: R, mut output: W) -> Result<()> {
    loop {
        let mut bytes = Vec::new();
        let count = std::io::Read::take(&mut input, MAX_PAYLOAD_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if count == 0 {
            return Ok(());
        }
        let response = if count > MAX_PAYLOAD_BYTES {
            if bytes.last() != Some(&b'\n') {
                input.skip_until(b'\n')?;
            }
            Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"MCP request exceeds payload limit"}}),
            )
        } else {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(request) => handle(&request),
                Err(_) => Some(
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"invalid JSON"}}),
                ),
            }
        };
        if let Some(response) = response {
            let mut bytes = serde_json::to_vec(&response)?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                bytes = serde_json::to_vec(
                    &json!({"jsonrpc":"2.0","id":response["id"],"error":{"code":-32603,"message":"MCP response exceeds payload limit"}}),
                )?;
            }
            output.write_all(&bytes)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}
