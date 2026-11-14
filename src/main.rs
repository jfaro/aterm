mod cli;
mod client;
mod detect;
mod keys;
mod paths;
mod protocol;
mod rpc;
mod server;

use std::process::ExitCode;

use clap::Parser;

/// A minimal terminal multiplexer for coding agents.
///
/// With no subcommand, attaches the TUI (starting the server if needed).
#[derive(Parser)]
#[command(name = "aterm", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("aterm: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        match cli.command {
            None => client::attach().await,
            Some(command) => cli::run(command).await,
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("aterm: {err:#}");
            ExitCode::FAILURE
        }
    }
}
