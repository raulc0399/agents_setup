//! Lays out one diagram for the headless UI harness (`ui-tests/run.sh`), which has no shell:
//! reads `{"document": …, "visible": [ids]}` on standard input, writes the `Diagram` JSON the
//! shell's `diagram_layout` command returns.
use prism_app_host::diagram::diagram;
use prism_core::model::{Document, Id};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::{Read, Write};

#[derive(Deserialize)]
struct Request {
    document: Document,
    visible: BTreeSet<Id>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input)?;
    let output = serde_json::to_string(&diagram(&request.document, &request.visible))?;
    std::io::stdout().write_all(output.as_bytes())?;
    Ok(())
}
