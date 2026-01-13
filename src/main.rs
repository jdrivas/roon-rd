mod cli;
mod server;
mod roon;

use clap::{Parser, Subcommand};
use simplelog::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use roon::RoonClient;

#[derive(Parser)]
#[command(name = "roon-rd")]
#[command(about = "Roon Remote Display - Query and serve Roon playback information", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Determine log level based on mode and verbose flag
    let log_level = match &cli.command {
        Commands::Query { .. } | Commands::Interactive => {
            // CLI mode: silent unless verbose
            if cli.verbose {
                LevelFilter::Debug
            } else {
                LevelFilter::Off
            }
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
        CombinedLogger::init(vec![
            TermLogger::new(
                log_level,
                Config::default(),
                TerminalMode::Mixed,
                ColorChoice::Auto,
            ),
        ])?;
    }

    // Create and initialize Roon client
    log::info!("Initializing Roon Remote Display...");
    let mut roon_client = RoonClient::new()?;
    roon_client.connect().await?;

    let client = Arc::new(Mutex::new(roon_client));

    // Handle commands
    match cli.command {
        Commands::Query { args } => {
            let query_string = args.join(" ");
            cli::handle_query(client, &query_string).await?;
        }
        Commands::Server { port } => {
            server::start_server(client, port).await?;
        }
        Commands::Interactive => {
            cli::handle_interactive(client).await?;
        }
    }

    Ok(())
}
