use std::sync::{Arc, Mutex as StdMutex};
use std::path::PathBuf;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use tokio::sync::Mutex;
use rustyline::error::ReadlineError;
use rustyline::{Editor, Config, CompletionType};
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::Helper;
use crate::roon::RoonClient;
use crate::upnp;
use simplelog::*;
use colored::Colorize;
use chrono::Local;

/// Command completer for interactive mode
struct CommandCompleter {
    commands: Vec<String>,
}

impl CommandCompleter {
    fn new(include_roon_commands: bool) -> Self {
        let mut commands = vec![
            // General commands
            "help".to_string(),
            "quit".to_string(),
            "exit".to_string(),
            "verbose".to_string(),
            "version".to_string(),
            // UPnP commands
            "upnp-discover".to_string(),
            "upnp-renderers".to_string(),
            "upnp-info".to_string(),
            "upnp-xml".to_string(),
            "upnp-service".to_string(),
            "upnp-position".to_string(),
            "upnp-state".to_string(),
        ];

        if include_roon_commands {
            // Roon commands
            commands.extend(vec![
                "status".to_string(),
                "zones".to_string(),
                "now-playing".to_string(),
                "play".to_string(),
                "pause".to_string(),
                "stop".to_string(),
                "mute".to_string(),
            ]);
        }

        commands.sort();
        Self { commands }
    }
}

impl Completer for CommandCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let start = line[..pos].rfind(' ').map(|i| i + 1).unwrap_or(0);
        let prefix = &line[start..pos];

        let matches: Vec<Pair> = self
            .commands
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| Pair {
                display: cmd.clone(),
                replacement: cmd.clone(),
            })
            .collect();

        Ok((start, matches))
    }
}

impl Hinter for CommandCompleter {
    type Hint = String;
}

impl Highlighter for CommandCompleter {}

impl Validator for CommandCompleter {}

impl Helper for CommandCompleter {}

/// Format duration in mm:ss format
fn format_duration(seconds: u32) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}

/// Get the path to the history file
fn get_history_file_path() -> Option<PathBuf> {
    if let Some(home_dir) = dirs::home_dir() {
        let history_path = home_dir.join(".roon-rd_history");
        Some(history_path)
    } else {
        None
    }
}

/// Load command history from file
fn load_history() -> Vec<String> {
    if let Some(history_path) = get_history_file_path() {
        if history_path.exists() {
            if let Ok(file) = fs::File::open(&history_path) {
                let reader = BufReader::new(file);
                return reader.lines()
                    .filter_map(|line| line.ok())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Save command history to file
fn save_history(history: &[String]) -> Result<(), std::io::Error> {
    if let Some(history_path) = get_history_file_path() {
        let mut file = fs::File::create(&history_path)?;
        for line in history {
            writeln!(file, "{}", line)?;
        }
    }
    Ok(())
}

/// Truncate history to last N entries and save
fn truncate_and_save_history(history: &[String], max_entries: usize) -> Result<(), std::io::Error> {
    let start = if history.len() > max_entries {
        history.len() - max_entries
    } else {
        0
    };
    save_history(&history[start..])
}

/// Output destination for commands
enum OutputDest {
    Stdout,
    Buffer(Arc<StdMutex<crate::tui::MessageBuffer>>),
}

impl OutputDest {
    fn writeln(&self, line: String) {
        match self {
            OutputDest::Stdout => println!("{}", line),
            OutputDest::Buffer(buffer) => {
                if let Ok(mut buf) = buffer.lock() {
                    buf.push(line);
                }
            }
        }
    }
}

/// Execute a query command against the Roon client (or UPnP-only commands)
async fn execute_query(client: Option<&RoonClient>, query_type: &str, _verbose: bool) -> Result<(), String> {
    execute_query_with_dest(client, query_type, OutputDest::Stdout).await
}

/// Execute a query command with output to buffer (for TUI)
async fn execute_query_to_buffer(
    client: Option<&RoonClient>,
    query_type: &str,
    buffer: Arc<StdMutex<crate::tui::MessageBuffer>>
) -> Result<(), String> {
    execute_query_with_dest(client, query_type, OutputDest::Buffer(buffer)).await
}

/// Execute a query command with custom output destination
async fn execute_query_with_dest(client: Option<&RoonClient>, query_type: &str, out: OutputDest) -> Result<(), String> {
    // Note: Log level is now managed by the verbose command in interactive mode

    match query_type {
        "status" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let connected = client.is_connected().await;
            out.writeln("".to_string());
            if connected {
                let core_name = client.get_core_name().await;
                out.writeln("  Status: Connected".to_string());
                if let Some(name) = core_name {
                    out.writeln(format!("  Core:   {}", name));
                }
            } else {
                out.writeln("  Status: Not connected".to_string());
                out.writeln("".to_string());
                out.writeln("  Authorize the extension in Roon Settings > Extensions".to_string());
            }
            out.writeln("".to_string());
            Ok(())
        }
        "zones" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let zones = client.get_zones().await;

            if zones.is_empty() {
                out.writeln("".to_string());
                out.writeln("  No zones found.".to_string());
                if !client.is_connected().await {
                    out.writeln("  Not connected to Roon Core. Please authorize the extension.".to_string());
                } else {
                    out.writeln("  Connected but no active zones.".to_string());
                }
                out.writeln("".to_string());
            } else {
                out.writeln("".to_string());
                for zone in &zones {
                    // Zone name with state
                    let state_str = format!("{:?}", zone.state).to_lowercase();
                    out.writeln(format!("  {} ({})", zone.display_name, state_str));
                    out.writeln(format!("    ID: {}", zone.zone_id));

                    // Show outputs (devices in this zone) indented
                    for output in &zone.outputs {
                        if output.display_name != zone.display_name {
                            out.writeln(format!("    └─ {}", output.display_name));
                        }
                    }
                }
                out.writeln("".to_string());
            }
            Ok(())
        }
        "now-playing" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let zones = client.get_zones().await;

            if zones.is_empty() {
                out.writeln("".to_string());
                out.writeln("  No zones found.".to_string());
                if !client.is_connected().await {
                    out.writeln("  Not connected to Roon Core. Please authorize the extension.".to_string());
                }
                out.writeln("".to_string());
            } else {
                let mut playing_count = 0;

                out.writeln("".to_string());
                for zone in &zones {
                    if let Some(now_playing) = &zone.now_playing {
                        playing_count += 1;

                        let state_str = format!("{:?}", zone.state).to_lowercase();
                        let three_line = &now_playing.three_line;

                        // Zone header
                        out.writeln(format!("  {} ({})", zone.display_name, state_str));
                        out.writeln("  ─────────────────────────────────────".to_string());

                        // Track info
                        out.writeln(format!("    {}", three_line.line1));
                        if !three_line.line2.is_empty() {
                            out.writeln(format!("    {}", three_line.line2));
                        }
                        if !three_line.line3.is_empty() {
                            out.writeln(format!("    {}", three_line.line3));
                        }

                        // Progress bar and time
                        if let Some(length) = now_playing.length {
                            // seek_position is in seconds (not milliseconds)
                            let position = now_playing.seek_position
                                .map(|p| p as u32)
                                .unwrap_or(0);

                            let progress = if length > 0 {
                                (position as f32 / length as f32 * 20.0) as usize
                            } else {
                                0
                            };
                            let bar = "━".repeat(progress) + &"─".repeat(20 - progress);

                            out.writeln("".to_string());
                            out.writeln(format!("    {} {} / {}", bar, format_duration(position), format_duration(length)));
                        }

                        out.writeln("".to_string());
                    }
                }

                if playing_count == 0 {
                    out.writeln("  No zones are currently playing.".to_string());
                    out.writeln("".to_string());
                }
            }
            Ok(())
        }
        "upnp-discover" => {
            out.writeln("".to_string());
            out.writeln("  Discovering UPnP devices (5 second timeout)...".to_string());
            out.writeln("".to_string());

            match upnp::discover_devices(5).await {
                Ok(devices) => {
                    if devices.is_empty() {
                        out.writeln("  No UPnP devices found.".to_string());
                    } else {
                        out.writeln(format!("  Found {} device(s):", devices.len()));
                        for (i, device) in devices.iter().enumerate() {
                            out.writeln("".to_string());
                            out.writeln(format!("  Device {}:", i + 1));
                            out.writeln(format!("    Location: {}", device.location));
                            if let Some(device_type) = &device.device_type {
                                out.writeln(format!("    Type: {}", device_type));
                            }

                            // Try to get detailed info
                            if let Ok(info) = upnp::get_device_info(&device.location).await {
                                out.writeln(format!("    Name: {}", info.friendly_name));
                                if let Some(mfr) = info.manufacturer {
                                    out.writeln(format!("    Manufacturer: {}", mfr));
                                }
                                if let Some(model) = info.model_name {
                                    out.writeln(format!("    Model: {}", model));
                                }
                            }
                        }
                    }
                    out.writeln("".to_string());
                    Ok(())
                }
                Err(e) => Err(format!("Discovery failed: {}", e))
            }
        }
        "upnp-renderers" => {
            out.writeln("".to_string());
            out.writeln("  Discovering UPnP MediaRenderers (5 second timeout)...".to_string());
            out.writeln("".to_string());

            match upnp::discover_media_renderers(5).await {
                Ok(devices) => {
                    if devices.is_empty() {
                        out.writeln("  No MediaRenderer devices found.".to_string());
                    } else {
                        out.writeln(format!("  Found {} MediaRenderer(s):", devices.len()));
                        for (i, device) in devices.iter().enumerate() {
                            out.writeln("".to_string());
                            out.writeln(format!("  Renderer {}:", i + 1));
                            out.writeln(format!("    Location: {}", device.location));

                            // Try to get detailed info
                            if let Ok(info) = upnp::get_device_info(&device.location).await {
                                out.writeln(format!("    Name: {}", info.friendly_name));
                                if let Some(mfr) = info.manufacturer {
                                    out.writeln(format!("    Manufacturer: {}", mfr));
                                }
                                if let Some(model) = info.model_name {
                                    out.writeln(format!("    Model: {}", model));
                                }
                            }
                        }
                    }
                    out.writeln("".to_string());
                    Ok(())
                }
                Err(e) => Err(format!("Discovery failed: {}", e))
            }
        }
        "verbose" => {
            // This is handled in interactive mode, not here
            Ok(())
        }
        "version" => {
            out.writeln("".to_string());
            out.writeln(format!("  roon-rd version {}", env!("CARGO_PKG_VERSION")));
            out.writeln("".to_string());
            Ok(())
        }
        "help" => {
            out.writeln("".to_string());
            out.writeln("  Available commands:".to_string());
            out.writeln("".to_string());
            out.writeln("  Roon Commands:".to_string());
            out.writeln("    status             - Show connection status".to_string());
            out.writeln("    zones              - List available zones".to_string());
            out.writeln("    now-playing        - Show currently playing tracks".to_string());
            out.writeln("    play <zone_id>     - Start playback in zone".to_string());
            out.writeln("    pause <zone_id>    - Pause playback in zone".to_string());
            out.writeln("    stop <zone_id>     - Stop playback in zone".to_string());
            out.writeln("    mute <zone_id>     - Toggle mute for zone".to_string());
            out.writeln("".to_string());
            out.writeln("  UPnP Commands:".to_string());
            out.writeln("    upnp-discover      - Discover all UPnP devices on network".to_string());
            out.writeln("    upnp-renderers     - Discover UPnP MediaRenderer devices".to_string());
            out.writeln("    upnp-info <url>    - Get detailed device information".to_string());
            out.writeln("    upnp-xml <url>     - Get raw device XML description".to_string());
            out.writeln("    upnp-service <url> <service> - Get service description XML (SCPD)".to_string());
            out.writeln("    upnp-position <url> - Get current playback position and metadata".to_string());
            out.writeln("    upnp-state <url>   - Get current playback state (playing/paused/stopped)".to_string());
            out.writeln("".to_string());
            out.writeln("  General:".to_string());
            out.writeln("    verbose            - Toggle verbose logging on/off".to_string());
            out.writeln("    version            - Show version information".to_string());
            out.writeln("    help               - Show this help message".to_string());
            out.writeln("    quit               - Exit interactive mode".to_string());
            out.writeln("".to_string());
            Ok(())
        }
        "" => Ok(()),
        _ => {
            // Check if it's a UPnP command with URL argument
            let parts: Vec<&str> = query_type.split_whitespace().collect();

            if parts.len() >= 2 {
                let command = parts[0];
                let arg = parts[1..].join(" ");

                match command {
                    "upnp-info" => {
                        out.writeln("".to_string());
                        out.writeln("  Getting device information...".to_string());
                        out.writeln("".to_string());

                        match upnp::get_device_info(&arg).await {
                            Ok(info) => {
                                out.writeln("  Device Information:".to_string());
                                out.writeln(format!("    Name: {}", info.friendly_name));
                                out.writeln(format!("    Type: {}", info.device_type));
                                if let Some(mfr) = info.manufacturer {
                                    out.writeln(format!("    Manufacturer: {}", mfr));
                                }
                                if let Some(model) = info.model_name {
                                    out.writeln(format!("    Model: {}", model));
                                }
                                if let Some(model_num) = info.model_number {
                                    out.writeln(format!("    Model Number: {}", model_num));
                                }
                                if let Some(serial) = info.serial_number {
                                    out.writeln(format!("    Serial: {}", serial));
                                }
                                if !info.services.is_empty() {
                                    out.writeln("".to_string());
                                    out.writeln("  Available Services:".to_string());
                                    for service in &info.services {
                                        out.writeln(format!("    - {}", service));
                                    }
                                }
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get device info: {}", e))
                        }
                    }
                    "upnp-xml" => {
                        out.writeln("".to_string());
                        out.writeln("  Fetching raw device XML...".to_string());
                        out.writeln("".to_string());

                        match upnp::get_device_xml(&arg).await {
                            Ok(xml) => {
                                out.writeln("  Raw Device XML:".to_string());
                                out.writeln("".to_string());
                                // Split by lines and indent each line
                                for line in xml.lines() {
                                    out.writeln(format!("  {}", line));
                                }
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get device XML: {}", e))
                        }
                    }
                    "upnp-position" => {
                        out.writeln("".to_string());
                        out.writeln("  Getting position info...".to_string());
                        out.writeln("".to_string());

                        match upnp::get_position_info(&arg).await {
                            Ok(info) => {
                                out.writeln("  Position Information:".to_string());
                                out.writeln(format!("    Track: {}", info.track));
                                out.writeln(format!("    Duration: {}", info.track_duration));
                                out.writeln(format!("    Position: {}", info.rel_time));
                                out.writeln(format!("    URI: {}", info.track_uri));
                                out.writeln("".to_string());

                                // Try to parse audio format from metadata
                                if !info.track_metadata.is_empty() {
                                    out.writeln("  Track Metadata (DIDL-Lite):".to_string());
                                    out.writeln(format!("    {}", info.track_metadata.chars().take(200).collect::<String>()));
                                    if info.track_metadata.len() > 200 {
                                        out.writeln("    ... (truncated)".to_string());
                                    }
                                    out.writeln("".to_string());

                                    if let Some(format) = upnp::parse_audio_format(&info.track_metadata) {
                                        out.writeln("  Audio Format:".to_string());
                                        if let Some(sr) = format.sample_rate {
                                            out.writeln(format!("    Sample Rate: {} Hz", sr));
                                        }
                                        if let Some(bits) = format.bits_per_sample {
                                            out.writeln(format!("    Bit Depth: {} bits", bits));
                                        }
                                        if let Some(ch) = format.channels {
                                            out.writeln(format!("    Channels: {}", ch));
                                        }
                                        if let Some(br) = format.bitrate {
                                            out.writeln(format!("    Bitrate: {} bps", br));
                                        }
                                        out.writeln("".to_string());
                                    }
                                }
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get position info: {}", e))
                        }
                    }
                    "upnp-state" => {
                        out.writeln("".to_string());
                        out.writeln("  Getting transport state...".to_string());
                        out.writeln("".to_string());

                        match upnp::get_transport_info(&arg).await {
                            Ok(info) => {
                                out.writeln("  Transport State:".to_string());
                                out.writeln(format!("    State: {}", info.current_transport_state));
                                out.writeln(format!("    Status: {}", info.current_transport_status));
                                out.writeln(format!("    Speed: {}", info.current_speed));
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get transport state: {}", e))
                        }
                    }
                    "upnp-service" => {
                        // Need at least 3 parts: command, url, service_type
                        if parts.len() < 3 {
                            return Err("Usage: upnp-service <device_url> <service_type>\n\nExample: upnp-service http://192.168.1.100:9000/description.xml AVTransport:2\n\nFirst run 'upnp-renderers' to get the device URL, then use that URL with the service name from 'upnp-info'.".to_string());
                        }

                        let device_url = parts[1];
                        let service_type = parts[2];

                        // Validate device URL format
                        if !device_url.starts_with("http://") && !device_url.starts_with("https://") {
                            return Err(format!("Invalid device URL: '{}'\n\nThe device URL must be an HTTP URL (e.g., http://192.168.1.100:9000/description.xml).\nRun 'upnp-renderers' to see available device URLs.", device_url));
                        }

                        out.writeln("".to_string());
                        out.writeln(format!("  Fetching service description for {}...", service_type));
                        out.writeln("".to_string());

                        match upnp::get_service_description(device_url, service_type).await {
                            Ok(xml) => {
                                out.writeln("  Service Description XML (SCPD):".to_string());
                                out.writeln("".to_string());
                                // Split by lines and indent each line
                                for line in xml.lines() {
                                    out.writeln(format!("  {}", line));
                                }
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get service description: {}", e))
                        }
                    }
                    _ => {}
                }
            }

            // Check if it's a control command with zone_id
            let parts: Vec<&str> = query_type.split_whitespace().collect();
            if parts.len() == 2 {
                let command = parts[0];
                let zone_id = parts[1];

                match command {
                    "play" | "pause" | "stop" => {
                        let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
                        match client.control_zone(zone_id, command).await {
                            Ok(_) => {
                                out.writeln("".to_string());
                                out.writeln(format!("  {} command sent to zone", command));
                                out.writeln("".to_string());
                                Ok(())
                            }
                            Err(e) => Err(format!("Control failed: {}", e))
                        }
                    }
                    "mute" => {
                        let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
                        // Toggle mute - we'll use true to mute (server handler will toggle)
                        match client.mute_output(zone_id, true).await {
                            Ok(_) => {
                                out.writeln("".to_string());
                                out.writeln("  Mute toggled for zone".to_string());
                                out.writeln("".to_string());
                                Ok(())
                            }
                            Err(e) => Err(format!("Mute failed: {}", e))
                        }
                    }
                    _ => Err(format!("Unknown command: {}\nType 'help' for available commands.", query_type))
                }
            } else if parts.len() == 1 && (parts[0] == "play" || parts[0] == "pause" || parts[0] == "stop" || parts[0] == "mute") {
                Err(format!("Usage: {} <zone_id>\nUse 'zones' to see available zone IDs.", parts[0]))
            } else {
                Err(format!("Unknown command: {}\nType 'help' for available commands.", query_type))
            }
        }
    }
}

/// Handle CLI query commands
pub async fn handle_query(client: Option<Arc<Mutex<RoonClient>>>, query_type: &str, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Wait for authorization if we have a client
    if let Some(ref client) = client {
        let client = client.lock().await;
        client.wait_for_authorization(15, None).await;

        // Give a brief moment for zone data to arrive after connection
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // Execute query with optional client reference
    if let Some(client) = client {
        let client = client.lock().await;
        execute_query(Some(&client), query_type, verbose).await.map_err(|e| e.into())
    } else {
        execute_query(None, query_type, verbose).await.map_err(|e| e.into())
    }
}

/// Handle interactive mode - read commands from stdin with history support
pub async fn handle_interactive(client: Option<Arc<Mutex<RoonClient>>>, verbose_flag: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize verbose state and track current log level
    // The logger was initialized at Trace level in main.rs to allow full dynamic control
    // Set initial log level based on -v flag: Info if -v was passed, Off otherwise
    let mut verbose = verbose_flag;
    let mut current_log_level = if verbose_flag {
        LevelFilter::Info
    } else {
        LevelFilter::Off
    };
    log::set_max_level(current_log_level);

    println!();
    if client.is_some() {
        println!("Roon Remote Display - Interactive Mode");
        println!("Type 'help' for available commands, 'quit' to exit.");
        println!("Use arrow keys or Ctrl-P/Ctrl-N to navigate command history.");
        println!("Press Tab for command completion.");
        if verbose {
            println!("Logging: {} (use '{}' command to change level)", "info".bold().green(), "verbose".cyan());
        } else {
            println!("Logging: {} (use '{}' command to enable)", "off".bold(), "verbose".cyan());
        }
        println!();
        println!("Please enable the extension in Roon Settings > Extensions.");
        println!();
    } else {
        println!("Roon Remote Display - UPnP-Only Interactive Mode");
        println!("Type 'help' for available commands, 'quit' to exit.");
        println!("Use arrow keys or Ctrl-P/Ctrl-N to navigate command history.");
        println!("Press Tab for command completion.");
        if verbose {
            println!("Logging: {} (use '{}' command to change level)", "info".bold().green(), "verbose".cyan());
        } else {
            println!("Logging: {} (use '{}' command to enable)", "off".bold(), "verbose".cyan());
        }
        println!();
        println!("Note: Roon commands are disabled (--upnp-only mode)");
        println!();
    }

    // Create readline editor with history and completion support
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();

    let helper = CommandCompleter::new(client.is_some());
    let mut rl = Editor::with_config(config)?;
    rl.set_helper(Some(helper));

    // Load history from file
    let loaded_history = load_history();
    for entry in &loaded_history {
        let _ = rl.add_history_entry(entry.as_str());
    }

    loop {
        // Build dynamic prompt with core name, connection status, and log level
        let log_level_str = match current_log_level {
            LevelFilter::Off => "off",
            LevelFilter::Error => "error",
            LevelFilter::Warn => "warn",
            LevelFilter::Info => "info",
            LevelFilter::Debug => "debug",
            LevelFilter::Trace => "trace",
        };

        let prompt = if let Some(ref client) = client {
            let client = client.lock().await;
            if let Some(core_name) = client.get_core_name().await {
                // Connected - show core name in dark blue
                format!("{}: {}> ", core_name.blue().bold(), log_level_str.cyan())
            } else {
                // Not connected - show "disconnected" in red
                format!("{}: {}> ", "disconnected".red().bold(), log_level_str.cyan())
            }
        } else {
            // UPnP-only mode - show "UPnP" in green
            format!("{}: {}> ", "UPnP".green().bold(), log_level_str.cyan())
        };

        // Read line with history support
        let readline = rl.readline(&prompt);

        match readline {
            Ok(line) => {
                let command = line.trim();

                // Skip empty lines but don't add to history
                if command.is_empty() {
                    continue;
                }

                // Add to history
                let _ = rl.add_history_entry(command);

                // Save history to file after each command
                let current_history: Vec<String> = rl.history().iter().map(|s| s.to_string()).collect();
                let _ = save_history(&current_history);

                // Check for quit command
                if command == "quit" || command == "exit" {
                    break;
                }

                // Check for verbose command with optional level argument
                if command.starts_with("verbose") {
                    let parts: Vec<&str> = command.split_whitespace().collect();

                    if parts.len() == 1 {
                        // Toggle behavior when no argument provided
                        verbose = !verbose;
                        println!();
                        if verbose {
                            println!("  {} logging {}", "Verbose".bold().green(), "enabled".bold().green());
                            current_log_level = LevelFilter::Info;
                            log::set_max_level(current_log_level);
                        } else {
                            println!("  {} logging {}", "Verbose".bold(), "disabled".bold());
                            current_log_level = LevelFilter::Off;
                            log::set_max_level(current_log_level);
                        }
                        println!();
                    } else if parts.len() == 2 {
                        // Set specific log level
                        let level_str = parts[1].to_lowercase();
                        let new_level = match level_str.as_str() {
                            "off" => {
                                verbose = false;
                                LevelFilter::Off
                            },
                            "error" => {
                                verbose = true;
                                LevelFilter::Error
                            },
                            "warn" => {
                                verbose = true;
                                LevelFilter::Warn
                            },
                            "info" => {
                                verbose = true;
                                LevelFilter::Info
                            },
                            "debug" => {
                                verbose = true;
                                LevelFilter::Debug
                            },
                            "trace" => {
                                verbose = true;
                                LevelFilter::Trace
                            },
                            _ => {
                                println!();
                                println!("  {} Invalid log level. Use: off, error, warn, info, debug, trace", "Error:".bold().red());
                                println!();
                                continue;
                            }
                        };

                        current_log_level = new_level;
                        log::set_max_level(current_log_level);
                        println!();
                        if verbose {
                            println!("  {} logging set to {}", "Verbose".bold().green(), level_str.bold().green());
                        } else {
                            println!("  {} logging {}", "Verbose".bold(), "disabled".bold());
                        }
                        println!();
                    } else {
                        println!();
                        println!("  {} Usage: verbose [off|error|warn|info|debug|trace]", "Error:".bold().red());
                        println!();
                    }
                    continue;
                }

                // Execute the command
                if let Some(ref client) = client {
                    let client = client.lock().await;
                    if let Err(e) = execute_query(Some(&client), command, verbose).await {
                        println!("  Error: {}", e);
                        println!();
                    }
                } else {
                    if let Err(e) = execute_query(None, command, verbose).await {
                        println!("  Error: {}", e);
                        println!();
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C - just print newline and continue
                println!();
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D - exit gracefully
                println!();
                break;
            }
            Err(err) => {
                println!("  Error: {:?}", err);
                break;
            }
        }
    }

    // Truncate history to last 100 entries and save on exit
    let final_history: Vec<String> = rl.history().iter().map(|s| s.to_string()).collect();
    let _ = truncate_and_save_history(&final_history, 100);

    Ok(())
}

/// Handle TUI mode - interactive mode with fixed prompt and scrolling output
pub async fn handle_tui(client: Option<Arc<Mutex<RoonClient>>>, verbose_flag: bool) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Mutex as StdMutex;
    use crate::tui;

    // Create message buffer
    let message_buffer = Arc::new(StdMutex::new(tui::MessageBuffer::new(1000)));

    // Set up TUI logger with shared mutable log level
    let current_log_level = Arc::new(StdMutex::new(if verbose_flag {
        LevelFilter::Info
    } else {
        LevelFilter::Off
    }));

    // Configure logging to filter out noisy dependencies
    let log_config = simplelog::ConfigBuilder::new()
        .add_filter_ignore_str("rustyline")
        .add_filter_ignore_str("hyper")
        .add_filter_ignore_str("roon_api::moo")
        .add_filter_ignore_str("tokio_tungstenite")
        .build();

    // Initialize TUI logger
    let tui_logger = tui::TuiLogger::new(message_buffer.clone(), current_log_level.clone(), log_config);
    simplelog::CombinedLogger::init(vec![Box::new(tui_logger)])?;

    // Add welcome messages
    {
        let mut buffer = message_buffer.lock().unwrap();
        buffer.push("Roon Remote Display - Terminal UI Mode".to_string());
        buffer.push("Use arrow keys or Ctrl-P/Ctrl-N to navigate command history.".to_string());
        buffer.push("".to_string());

        if client.is_some() {
            buffer.push("Please enable the extension in Roon Settings > Extensions.".to_string());
        } else {
            buffer.push("UPnP-only mode - Roon commands disabled".to_string());
        }

        if verbose_flag {
            buffer.push(format!("Logging: {} (use '{}' command to change level)", "info".to_string(), "verbose"));
        } else {
            buffer.push(format!("Logging: {} (use '{}' command to enable)", "off".to_string(), "verbose"));
        }
        buffer.push("".to_string());
    }

    // Create exit flag
    let exit_flag = Arc::new(StdMutex::new(false));
    let exit_flag_for_handler = exit_flag.clone();
    let exit_flag_for_updater = exit_flag.clone();

    // Clone client for use in command handler
    let client_clone = client.clone();
    let buffer_for_commands = message_buffer.clone();
    let log_level_for_prompt = current_log_level.clone();
    let log_level_for_handler = current_log_level.clone();

    // Get initial prompt base
    let prompt_base = Arc::new(StdMutex::new(if client.is_some() {
        // Try to get core name if connected
        if let Some(ref cl) = client {
            let roon_client = cl.lock().await;
            roon_client.get_core_name().await.unwrap_or_else(|| "disconnected".to_string())
        } else {
            "disconnected".to_string()
        }
    } else {
        "UPnP".to_string()
    }));

    let prompt_base_for_fn = prompt_base.clone();
    let prompt_base_for_updater = prompt_base.clone();
    let client_for_updater = client.clone();

    // Spawn background task to update prompt based on connection status
    if client.is_some() {
        tokio::spawn(async move {
            loop {
                // Check if we should exit
                if let Ok(should_exit) = exit_flag_for_updater.lock() {
                    if *should_exit {
                        break;
                    }
                }

                // Update prompt base with current connection status
                if let Some(ref cl) = client_for_updater {
                    let roon_client = cl.lock().await;
                    let new_base = if roon_client.is_connected().await {
                        roon_client.get_core_name().await.unwrap_or_else(|| "disconnected".to_string())
                    } else {
                        "disconnected".to_string()
                    };

                    if let Ok(mut base) = prompt_base_for_updater.lock() {
                        *base = new_base;
                    }
                }

                // Check every second
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        });
    }

    // Create prompt function
    let prompt_fn = move || {
        let base = prompt_base_for_fn.lock().unwrap().clone();
        let level = *log_level_for_prompt.lock().unwrap();
        let log_level_str = match level {
            LevelFilter::Off => "off",
            LevelFilter::Error => "error",
            LevelFilter::Warn => "warn",
            LevelFilter::Info => "info",
            LevelFilter::Debug => "debug",
            LevelFilter::Trace => "trace",
        };
        format!("{}: {}> ", base, log_level_str)
    };

    // Build commands list for completion
    let mut commands = vec![
        // General commands
        "help".to_string(),
        "quit".to_string(),
        "exit".to_string(),
        "verbose".to_string(),
        "version".to_string(),
        "clear".to_string(),
        "test".to_string(),
        // UPnP commands
        "upnp-discover".to_string(),
        "upnp-renderers".to_string(),
        "upnp-info".to_string(),
        "upnp-xml".to_string(),
        "upnp-service".to_string(),
        "upnp-position".to_string(),
        "upnp-state".to_string(),
    ];

    if client.is_some() {
        // Roon commands
        commands.extend(vec![
            "status".to_string(),
            "zones".to_string(),
            "now-playing".to_string(),
            "play".to_string(),
            "pause".to_string(),
            "stop".to_string(),
            "mute".to_string(),
        ]);
    }
    commands.sort();

    // Run TUI with async command handler
    tui::run_tui_async(message_buffer, prompt_fn, move |command| {
        let buffer_for_commands = buffer_for_commands.clone();
        let client_clone = client_clone.clone();
        let log_level_for_handler = log_level_for_handler.clone();
        let exit_flag_for_handler = exit_flag_for_handler.clone();

        async move {
            // Echo the command to the output with timestamp
            {
                let mut buffer = buffer_for_commands.lock().unwrap();
                let now = Local::now();
                buffer.push(format!("[{}] > {}", now.format("%H:%M:%S"), command));
                buffer.push("".to_string());
            } // Lock dropped here so TUI can render the command echo immediately

            // Yield to allow TUI to render the command echo before executing
            tokio::task::yield_now().await;

            // Handle verbose command first (since it's synchronous)
            if command.starts_with("verbose") {
                let mut buffer = buffer_for_commands.lock().unwrap();
                let parts: Vec<&str> = command.split_whitespace().collect();

                if parts.len() == 1 {
                    // Toggle behavior
                    let mut level = log_level_for_handler.lock().unwrap();
                    let new_level = if *level == LevelFilter::Off {
                        LevelFilter::Info
                    } else {
                        LevelFilter::Off
                    };
                    *level = new_level;
                    log::set_max_level(new_level);

                    buffer.push("".to_string());
                    if new_level != LevelFilter::Off {
                        buffer.push(format!("  Verbose logging enabled ({})", format!("{:?}", new_level).to_lowercase()));
                    } else {
                        buffer.push("  Verbose logging disabled".to_string());
                    }
                    buffer.push("".to_string());
                } else if parts.len() == 2 {
                    let level_str = parts[1].to_lowercase();
                    let new_level = match level_str.as_str() {
                        "off" => LevelFilter::Off,
                        "error" => LevelFilter::Error,
                        "warn" => LevelFilter::Warn,
                        "info" => LevelFilter::Info,
                        "debug" => LevelFilter::Debug,
                        "trace" => LevelFilter::Trace,
                        _ => {
                            buffer.push("".to_string());
                            buffer.push("  Error: Invalid log level. Use: off, error, warn, info, debug, trace".to_string());
                            buffer.push("".to_string());
                            return;
                        }
                    };

                    let mut level = log_level_for_handler.lock().unwrap();
                    *level = new_level;
                    log::set_max_level(new_level);
                    buffer.push("".to_string());
                    buffer.push(format!("  Verbose logging set to {}", level_str));
                    buffer.push("".to_string());
                }
                return;
            }

            // Handle clear command
            if command.trim() == "clear" {
                let mut buffer = buffer_for_commands.lock().unwrap();
                buffer.clear();
                return;
            }

            // Handle test command for debugging
            if command.trim() == "test" {
                let mut buffer = buffer_for_commands.lock().unwrap();
                buffer.push("".to_string());
                buffer.push("  Test output line 1".to_string());
                buffer.push("  Test output line 2".to_string());
                buffer.push("  Test output line 3".to_string());
                buffer.push("".to_string());
                return;
            }

            // Handle quit/exit commands
            if command.trim() == "quit" || command.trim() == "exit" {
                let mut buffer = buffer_for_commands.lock().unwrap();
                buffer.push("".to_string());
                buffer.push("  Exiting...".to_string());
                if let Ok(mut flag) = exit_flag_for_handler.lock() {
                    *flag = true;
                }
                return;
            }

            // Execute other commands through existing handler with buffer output
            // Lock already released above

            let client_ref = if let Some(ref cl) = client_clone {
                let guard = cl.lock().await;
                Some(guard)
            } else {
                None
            };

            let result = execute_query_to_buffer(
                client_ref.as_deref(),
                command.trim(),
                buffer_for_commands.clone()
            ).await;

            // Handle errors
            if let Err(e) = result {
                let mut buffer = buffer_for_commands.lock().unwrap();
                buffer.push("".to_string());
                buffer.push(format!("  Error: {}", e));
                buffer.push("".to_string());
            }
        }
    }, exit_flag, commands).await?;

    Ok(())
}
