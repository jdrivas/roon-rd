mod cli;
mod server;
mod roon;
mod upnp;
mod tui;

use clap::{Parser, Subcommand};
use simplelog::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use roon::RoonClient;

#[derive(Parser)]
#[command(name = "roon-rd")]
#[command(about = "Roon Remote Display - Query and serve Roon playback information", long_about = None)]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// UPnP-only mode (don't connect to Roon)
    #[arg(long, global = true)]
    upnp_only: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Query Roon for information or control playback
    Query {
        /// Command and arguments: status, zones, now-playing, play <zone_id>, pause <zone_id>, stop <zone_id>, mute <zone_id>
        #[arg(value_name = "COMMAND", num_args = 1..)]
        args: Vec<String>,
    },
    /// Start web server mode
    Server {
        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,
    },
    /// Interactive mode - read commands from stdin
    Interactive,
    /// Terminal UI mode - interactive with fixed prompt
    Tui,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Determine log level based on mode and verbose flag
    let log_level = match &cli.command {
        Commands::Query { .. } | Commands::Interactive => {
            // CLI/Interactive mode: Always initialize at Trace level to allow dynamic control
            // The actual level will be controlled via log::set_max_level() in interactive mode
            LevelFilter::Trace
        }
        Commands::Tui => {
            // TUI mode: Don't initialize standard logger, we'll handle output via TUI
            LevelFilter::Off
        }
        Commands::Server { .. } => {
            // Server mode: info by default, debug if verbose
            if cli.verbose {
                LevelFilter::Debug
            } else {
                LevelFilter::Info
            }
        }
    };

    if log_level != LevelFilter::Off {
        // Configure logging to filter out noisy dependencies
        let config = ConfigBuilder::new()
            .add_filter_ignore_str("rustyline")  // Ignore rustyline debug messages
            .add_filter_ignore_str("hyper")       // Ignore hyper HTTP client debug messages
            .add_filter_ignore_str("roon_api::moo")  // Ignore roon_api ping messages
            .add_filter_ignore_str("tokio_tungstenite")  // Ignore WebSocket polling messages
            .build();

        CombinedLogger::init(vec![
            TermLogger::new(
                log_level,
                config,
                TerminalMode::Mixed,
                ColorChoice::Auto,
            ),
        ])?;
    }

    // Create and initialize Roon client (unless upnp-only mode)
    let client = if cli.upnp_only {
        // Create a dummy client that won't be used
        None
    } else {
        log::info!("Initializing Roon Remote Display...");
        let mut roon_client = RoonClient::new()?;
        roon_client.connect().await?;
        Some(Arc::new(Mutex::new(roon_client)))
    };

    // Handle commands
    match cli.command {
        Commands::Query { args } => {
            let query_string = args.join(" ");
            cli::handle_query(client, &query_string, cli.verbose).await?;
        }
        Commands::Server { port } => {
            if let Some(client) = client {
                server::start_server(client, port).await?;
            } else {
                return Err("Server mode requires Roon connection. Remove --upnp-only flag.".into());
            }
        }
        Commands::Interactive => {
            cli::handle_interactive(client, cli.verbose).await?;
        }
        Commands::Tui => {
            cli::handle_tui(client, cli.verbose).await?;
        }
    }

    Ok(())
}
