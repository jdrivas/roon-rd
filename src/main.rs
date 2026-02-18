mod cli;
mod config;
mod server;
mod roon;
mod upnp;
mod dcs;
mod tui;

use clap::{Parser, Subcommand, ValueEnum};
use simplelog::*;
use time::macros::format_description;
use std::sync::Arc;
use tokio::sync::Mutex;
use roon::RoonClient;

/// Log level options for CLI
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Off,
}

impl LogLevel {
    pub fn to_level_filter(self) -> LevelFilter {
        match self {
            LogLevel::Trace => LevelFilter::Trace,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Off => LevelFilter::Off,
        }
    }
}

/// Time format options for log timestamps
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum TimeFormat {
    Local,
    Utc,
}

#[derive(Parser)]
#[command(name = "roon-rd")]
#[command(about = "Roon Remote Display - Query and serve Roon playback information", long_about = None)]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Set log level (trace, debug, info, warn, error, off)
    #[arg(long, global = true, value_enum)]
    log_level: Option<LogLevel>,

    /// Set log timestamp format (local, utc)
    #[arg(long, global = true, value_enum, default_value = "local")]
    log_time: TimeFormat,

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
        #[arg(short, long)]
        port: Option<u16>,
        /// Artist image carousel interval in seconds
        #[arg(long)]
        carousel_interval: Option<u32>,
        /// Path to a timed interchange libretto JSON file
        #[arg(long)]
        libretto: Option<String>,
        /// Directory containing *.interchange.json files for multi-opera support
        #[arg(long)]
        libretto_dir: Option<String>,
        /// Path to config file (default: ~/.config/roon-rd/config.toml)
        #[arg(long)]
        config: Option<String>,
    },
    /// Interactive mode - read commands from stdin
    Interactive,
    /// Terminal UI mode - interactive with fixed prompt
    Tui,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Determine log level based on mode and --log-level flag
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
            // Server mode: default to info if --log-level not provided, respect explicit off
            match cli.log_level {
                Some(level) => level.to_level_filter(),
                None => LevelFilter::Info,
            }
        }
    };

    if log_level != LevelFilter::Off {
        // Configure logging to filter out noisy dependencies and show file:line for DEBUG
        let mut config_builder = ConfigBuilder::new();
        config_builder
            .add_filter_ignore_str("rustyline")  // Ignore rustyline debug messages
            .add_filter_ignore_str("hyper")       // Ignore hyper HTTP client debug messages
            .add_filter_ignore_str("roon_api::moo")  // Ignore roon_api ping messages
            .add_filter_ignore_str("tokio_tungstenite")  // Ignore WebSocket polling messages
            .add_filter_ignore_str("libmdns")  // Ignore libmdns "query type 65 is invalid" warnings
            .add_filter_ignore_str("mdns_sd")  // Ignore mdns-sd interface send errors
            .set_location_level(LevelFilter::Debug);  // Show file:line for DEBUG level and below

        // Set custom time format with timezone indicator
        // Format: 2024-01-15 14:30:45 PST or 2024-01-15 14:30:45 UTC
        let time_format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second] [offset_hour sign:mandatory][offset_minute]");
        config_builder.set_time_format_custom(time_format);

        // Set time offset based on --log-time flag
        // Default is UTC; set_time_offset_to_local() attempts to use local time
        if cli.log_time == TimeFormat::Local {
            let _ = config_builder.set_time_offset_to_local();
        }

        let config = config_builder.build();

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
            cli::handle_query(client, &query_string, cli.log_level.map_or(LevelFilter::Off, |l| l.to_level_filter())).await?;
        }
        Commands::Server { port, carousel_interval, libretto, libretto_dir, config: config_path } => {
            // Load layered config: defaults < config file < env vars < CLI args
            let app_config = config::load_config(
                config_path.as_deref(),
                config::CliOverrides {
                    port,
                    carousel_interval,
                    libretto,
                    libretto_dir,
                },
            ).map_err(|e| format!("Configuration error: {}", e))?;

            if let Some(client) = client {
                server::start_server(client, app_config).await?;
            } else {
                return Err("Server mode requires Roon connection. Remove --upnp-only flag.".into());
            }
        }
        Commands::Interactive => {
            cli::handle_interactive(client, cli.log_level.map_or(LevelFilter::Off, |l| l.to_level_filter()), cli.log_time == TimeFormat::Local).await?;
        }
        Commands::Tui => {
            cli::handle_tui(client, cli.log_level.map_or(LevelFilter::Off, |l| l.to_level_filter()), cli.log_time == TimeFormat::Local).await?;
        }
    }

    Ok(())
}
