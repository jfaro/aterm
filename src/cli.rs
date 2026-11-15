//! One-shot commands: thin wrappers over the socket API.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use serde_json::json;

use crate::detect::AgentState;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;
use crate::rpc::Connection;

#[derive(Subcommand)]
pub enum Command {
    /// Run the server in the foreground (normally started automatically).
    Server {
        /// What to do with the server; omit to run it.
        #[command(subcommand)]
        action: Option<ServerAction>,
    },

    /// Create, inspect and drive panes.
    Pane {
        #[command(subcommand)]
        action: PaneAction,
    },

    /// Wait on agent status.
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },

    /// Stream server events as JSON lines.
    Events,
}

#[derive(Subcommand)]
pub enum ServerAction {
    /// Stop the running server and every pane in it.
    Stop,
}

#[derive(Subcommand)]
pub enum PaneAction {
    /// Start a pane running COMMAND (or your shell).
    Create {
        /// Name shown in the sidebar; defaults to following the window title.
        #[arg(long)]
        name: Option<String>,

        /// Working directory; defaults to the current one.
        #[arg(long)]
        cwd: Option<PathBuf>,

        /// Command line to run, e.g. `claude --resume`.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// List panes.
    List {
        /// Print full pane details as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Print a pane's visible screen.
    Read {
        /// Pane to read.
        pane: PaneId,

        /// Only the last N non-blank lines.
        #[arg(long)]
        lines: Option<usize>,

        /// Include colors and cursor state as ANSI escapes.
        #[arg(long)]
        ansi: bool,
    },

    /// Type literal text into a pane.
    SendText {
        /// Pane to type into.
        pane: PaneId,

        /// Text to type, sent as-is.
        text: String,

        /// Press enter afterwards.
        #[arg(long)]
        enter: bool,
    },

    /// Press named keys, e.g. `enter`, `ctrl-c`, `alt-left`, `up`.
    SendKeys {
        /// Pane to send the keys to.
        pane: PaneId,

        /// Keys to press, in order.
        #[arg(required = true)]
        keys: Vec<String>,
    },

    /// Rename a pane.
    Rename {
        /// Pane to rename.
        pane: PaneId,

        /// New name; omit it to go back to automatic naming.
        name: Option<String>,
    },

    /// Move a pane to another position in the sidebar.
    Move {
        /// Pane to move.
        pane: PaneId,

        /// New position, counting from 1 as the sidebar does.
        #[arg(value_parser = clap::value_parser!(u32).range(1..))]
        position: u32,
    },

    /// Kill a pane's process and remove it.
    Close {
        /// Pane to close.
        pane: PaneId,
    },
}

#[derive(Subcommand)]
pub enum AgentAction {
    /// Block until the agent in PANE reaches STATUS.
    Wait {
        /// Pane whose agent to watch.
        pane: PaneId,

        /// Status to wait for.
        #[arg(long, value_enum)]
        status: AgentState,

        /// Give up after this many seconds.
        #[arg(long)]
        timeout: Option<f64>,
    },
}

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Server { action: None } => crate::server::run().await,
        Command::Server {
            action: Some(ServerAction::Stop),
        } => {
            let (conn, _) = Connection::connect().await?;
            conn.call("server.stop", json!({})).await?;
            Ok(())
        }
        Command::Pane { action } => {
            let (conn, _) = Connection::connect_or_spawn().await?;
            pane(&conn, action).await
        }
        Command::Agent {
            action:
                AgentAction::Wait {
                    pane,
                    status,
                    timeout,
                },
        } => {
            let (conn, _) = Connection::connect_or_spawn().await?;
            let timeout_ms = timeout.map(|secs| (secs * 1000.0) as u64);
            let info: PaneInfo = conn
                .call_as(
                    "agent.wait",
                    json!({ "pane": pane, "status": status, "timeout_ms": timeout_ms }),
                )
                .await?;
            println!("{}", serde_json::to_string(&info)?);
            Ok(())
        }
        Command::Events => {
            let (conn, mut events) = Connection::connect_or_spawn().await?;
            conn.call("events.subscribe", json!({})).await?;
            while let Some(event) = events.recv().await {
                println!("{}", serde_json::to_string(&event)?);
            }
            Ok(())
        }
    }
}

async fn pane(conn: &Arc<Connection>, action: PaneAction) -> Result<()> {
    match action {
        PaneAction::Create { name, cwd, command } => {
            let cwd = match cwd {
                Some(dir) => Some(std::fs::canonicalize(dir)?),
                None => std::env::current_dir().ok(),
            };
            let command = (!command.is_empty()).then(|| command.join(" "));
            let info: PaneInfo = conn
                .call_as(
                    "pane.create",
                    json!({ "command": command, "cwd": cwd, "name": name }),
                )
                .await?;
            println!("{}", info.id);
        }
        PaneAction::List { json } => {
            let panes: Vec<PaneInfo> = conn.call_as("pane.list", json!({})).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&panes)?);
            } else {
                print_pane_table(&panes);
            }
        }
        PaneAction::Read { pane, lines, ansi } => {
            let format = if ansi { "ansi" } else { "text" };
            let result = conn
                .call(
                    "pane.read",
                    json!({ "pane": pane, "lines": lines, "format": format }),
                )
                .await?;
            let body = result
                .get(format)
                .and_then(Value::as_str)
                .unwrap_or_default();
            println!("{body}");
        }
        PaneAction::SendText { pane, text, enter } => {
            conn.call("pane.send_text", json!({ "pane": pane, "text": text }))
                .await?;
            if enter {
                conn.call("pane.send_keys", json!({ "pane": pane, "keys": ["enter"] }))
                    .await?;
            }
        }
        PaneAction::SendKeys { pane, keys } => {
            conn.call("pane.send_keys", json!({ "pane": pane, "keys": keys }))
                .await?;
        }
        PaneAction::Rename { pane, name } => {
            conn.call("pane.rename", json!({ "pane": pane, "name": name }))
                .await?;
        }
        PaneAction::Move { pane, position } => {
            let index = position as usize - 1;
            conn.call("pane.move", json!({ "pane": pane, "index": index }))
                .await?;
        }
        PaneAction::Close { pane } => {
            conn.call("pane.close", json!({ "pane": pane })).await?;
        }
    }
    Ok(())
}

fn print_pane_table(panes: &[PaneInfo]) {
    println!(
        "{:<4} {:<16} {:<8} {:<8} COMMAND",
        "ID", "NAME", "AGENT", "STATUS"
    );
    for pane in panes {
        let status = serde_json::to_value(pane.status).ok();
        println!(
            "{:<4} {:<16} {:<8} {:<8} {}",
            pane.id,
            pane.name,
            pane.agent.as_deref().unwrap_or("-"),
            status.as_ref().and_then(Value::as_str).unwrap_or("?"),
            pane.command,
        );
    }
}
