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

    /// Starting width of the TUI sidebar, in columns.
    #[arg(
        long,
        default_value_t = client::DEFAULT_SIDEBAR_WIDTH,
        value_parser = clap::value_parser!(u16).range(i64::from(client::MIN_SIDEBAR_WIDTH)..),
    )]
    sidebar_width: u16,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Create a new runtime.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("aterm: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Run the aterm application.
    let result = runtime.block_on(async {
        match cli.command {
            None => client::attach(cli.sidebar_width).await,
            Some(command) => cli::run(command).await,
        }
    });

    // Handle server exit result.
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("aterm: {err:#}");
            ExitCode::FAILURE
        }
    }
}
