//! Entry points for one Codex or Claude conversation attached by its native plugin.
use anyhow::{Result, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use prism::bridge;
use prism::model::AgentKind;
use std::io;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    version,
    about = "A visual workspace beside an attached Codex or Claude CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Entry,
}

#[derive(Clone, Copy, ValueEnum)]
enum Agent {
    Codex,
    Claude,
}

impl Agent {
    fn kind(self) -> AgentKind {
        match self {
            Agent::Codex => AgentKind::Codex,
            Agent::Claude => AgentKind::Claude,
        }
    }
}

#[derive(Subcommand)]
enum Entry {
    /// Serve native MCP over standard input/output; configured by the Codex plugin.
    Mcp,
    /// Attach a native plugin to its running CLI conversation.
    #[command(hide = true)]
    Attach(Attach),
    /// Run the authenticated bridge for a protected binding; started by `attach`.
    #[command(hide = true)]
    Bridge(Bound),
}

#[derive(Args)]
struct Attach {
    #[arg(long, value_enum)]
    agent: Agent,
    #[arg(long, value_parser = conversation_id)]
    conversation: String,
}

#[derive(Args)]
struct Bound {
    #[arg(long)]
    binding: PathBuf,
}

fn conversation_id(value: &str) -> Result<String, String> {
    let id = Uuid::parse_str(value)
        .map_err(|_| "the conversation must be an exact native thread UUID")?;
    if id.is_nil() || id.to_string() != value {
        return Err("the conversation must be a canonical nonempty native thread UUID".into());
    }
    Ok(value.to_owned())
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Entry::Attach(options) => {
            ensure!(
                matches!(options.agent, Agent::Claude),
                "Codex attaches through its native MCP transport"
            );
            // SAFETY: getppid has no memory preconditions; the native Claude hook starts this child.
            let parent = unsafe { libc::getppid() } as u32;
            let binding = prism::attachment::ensure_attached(
                options.agent.kind(),
                &options.conversation,
                parent,
            )?;
            println!("{}", serde_json::json!({"binding": binding}));
            Ok(())
        }
        Entry::Bridge(options) => bridge::run(&options.binding),
        Entry::Mcp => prism::mcp::serve(io::stdin().lock(), io::stdout().lock()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("prism: {error:#}");
        std::process::exit(1);
    }
}
