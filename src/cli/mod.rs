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
use crate::dcs;
use simplelog::*;
use colored::Colorize;
use chrono::Local;

/// Default hostname for dCS devices when not specified
const DEFAULT_DCS_HOST: &str = "dcs-vivaldi.local";

/// Command metadata with name and help text
struct CommandInfo {
    name: &'static str,
    description: &'static str,
    usage: Option<&'static str>,
}

/// Central command definitions
fn get_command_definitions() -> Vec<CommandInfo> {
    vec![
        // General commands
        CommandInfo { name: "help", description: "Show available commands", usage: None },
        CommandInfo { name: "quit", description: "Exit interactive mode", usage: None },
        CommandInfo { name: "exit", description: "Exit interactive mode", usage: None },
        CommandInfo { name: "verbose", description: "Toggle verbose logging or set level", usage: Some("[off|error|warn|info|debug|trace]") },
        CommandInfo { name: "version", description: "Show version information", usage: None },

        // Roon commands
        CommandInfo { name: "status", description: "Show connection status", usage: None },
        CommandInfo { name: "reconnect", description: "Reconnect to Roon Core", usage: None },
        CommandInfo { name: "zones", description: "List available zones", usage: None },
        CommandInfo { name: "now-playing", description: "Show currently playing tracks", usage: None },
        CommandInfo { name: "queue", description: "Show queue for zone (defaults to first playing zone)", usage: Some("[zone]") },
        CommandInfo { name: "play", description: "Start playback in zone", usage: Some("<zone_id>") },
        CommandInfo { name: "pause", description: "Pause playback in zone", usage: Some("<zone_id>") },
        CommandInfo { name: "stop", description: "Stop playback in zone", usage: Some("<zone_id>") },
        CommandInfo { name: "mute", description: "Toggle mute for zone", usage: Some("<zone_id>") },

        // UPnP commands
        CommandInfo { name: "upnp-discover", description: "Discover all UPnP devices on network", usage: None },
        CommandInfo { name: "upnp-renderers", description: "Discover UPnP MediaRenderer devices", usage: None },
        CommandInfo { name: "upnp-info", description: "Get detailed device information", usage: Some("<url>") },
        CommandInfo { name: "upnp-xml", description: "Get raw device XML description", usage: Some("<url>") },
        CommandInfo { name: "upnp-service", description: "Get service description XML (SCPD)", usage: Some("<url> <service>") },
        CommandInfo { name: "upnp-position", description: "Get current playback position and metadata", usage: Some("<url>") },
        CommandInfo { name: "upnp-state", description: "Get current playback state (playing/paused/stopped)", usage: Some("<url>") },
        CommandInfo { name: "upnp-playing", description: "Get comprehensive now playing info (state, track, format)", usage: Some("<url>") },

        // dCS API commands
        CommandInfo { name: "dcs-playing", description: "Get current playback info (track, artist, album, format)", usage: Some("<host>") },
        CommandInfo { name: "dcs-format", description: "Get current audio format (sample rate, bit depth, input)", usage: Some("<host>") },
        CommandInfo { name: "dcs-settings", description: "Get device settings (display, sync mode)", usage: Some("<host>") },
        CommandInfo { name: "dcs-upsampler", description: "Get upsampler settings (output rate, filter)", usage: Some("<host>") },
        CommandInfo { name: "dcs-inputs", description: "Get current and available digital inputs", usage: Some("<host>") },
        CommandInfo { name: "dcs-playmode", description: "Get current play mode (Network, USB, etc)", usage: Some("<host>") },
        CommandInfo { name: "dcs-menu", description: "Get available menu options for device", usage: Some("<host>") },
        CommandInfo { name: "dcs-set-brightness", description: "Set display brightness (0-4)", usage: Some("<host> <level>") },
        CommandInfo { name: "dcs-set-display", description: "Set display mode (on/off)", usage: Some("<host> <on|off>") },
    ]
}

/// Command completer for interactive mode
struct CommandCompleter {
    commands: Vec<String>,
}

impl CommandCompleter {
    fn new(include_roon_commands: bool) -> Self {
        let definitions = get_command_definitions();
        let roon_commands = ["status", "reconnect", "zones", "now-playing", "queue", "play", "pause", "stop", "mute"];

        let commands: Vec<String> = definitions
            .iter()
            .filter(|cmd| {
                // Include all non-Roon commands, or include Roon commands if requested
                !roon_commands.contains(&cmd.name) || include_roon_commands
            })
            .map(|cmd| cmd.name.to_string())
            .collect();

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
        query if query.starts_with("queue") => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let zones = client.get_zones().await;

            if zones.is_empty() {
                out.writeln("".to_string());
                out.writeln("  No zones found.".to_string());
                if !client.is_connected().await {
                    out.writeln("  Not connected to Roon Core. Please authorize the extension.".to_string());
                }
                out.writeln("".to_string());
                return Ok(());
            }

            // Parse command to check for zone name argument
            let parts: Vec<&str> = query_type.split_whitespace().collect();

            // If a specific zone is specified, show queue for that zone
            // Otherwise show queue for the first zone with now_playing
            let target_zone = if parts.len() > 1 {
                let zone_name = parts[1..].join(" ");
                zones.iter().find(|z| z.display_name.to_lowercase().contains(&zone_name.to_lowercase()))
            } else {
                zones.iter().find(|z| z.now_playing.is_some())
            };

            if let Some(zone) = target_zone {
                out.writeln("".to_string());
                out.writeln(format!("  Queue for: {}", zone.display_name));
                out.writeln("  ─────────────────────────────────────".to_string());
                out.writeln("".to_string());

                // Subscribe to queue and wait for data
                client.subscribe_to_queue(&zone.zone_id).await;

                // Get the queue
                if let Some(queue_items) = client.get_queue(&zone.zone_id).await {
                    if queue_items.is_empty() {
                        out.writeln("  Queue is empty.".to_string());
                    } else {
                        for (i, item) in queue_items.iter().enumerate() {
                            let two_line = &item.two_line;
                            let num = format!("{:3}.", i + 1);

                            // Format track info
                            out.writeln(format!("  {} {}", num, two_line.line1));
                            if !two_line.line2.is_empty() {
                                out.writeln(format!("       {}", two_line.line2));
                            }

                            // Show length if available (length is u32 seconds)
                            if item.length > 0 {
                                out.writeln(format!("       Duration: {}", format_duration(item.length)));
                            }

                            out.writeln("".to_string());
                        }
                        out.writeln(format!("  Total: {} track{}", queue_items.len(), if queue_items.len() == 1 { "" } else { "s" }));
                    }
                } else {
                    out.writeln("  Could not retrieve queue.".to_string());
                }
                out.writeln("".to_string());
            } else {
                out.writeln("".to_string());
                if parts.len() > 1 {
                    out.writeln(format!("  Zone '{}' not found.", parts[1..].join(" ")));
                } else {
                    out.writeln("  No zones are currently playing.".to_string());
                    out.writeln("  Usage: queue [zone name]".to_string());
                }
                out.writeln("".to_string());
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
            let definitions = get_command_definitions();

            out.writeln("".to_string());
            out.writeln("  Available commands:".to_string());
            out.writeln("".to_string());

            // Group commands by category
            let general_cmds = ["help", "quit", "exit", "verbose", "version"];
            let roon_cmds = ["status", "reconnect", "zones", "now-playing", "queue", "play", "pause", "stop", "mute"];
            let upnp_cmds: Vec<_> = definitions.iter().filter(|c| c.name.starts_with("upnp-")).collect();
            let dcs_cmds: Vec<_> = definitions.iter().filter(|c| c.name.starts_with("dcs-")).collect();

            // Roon Commands
            out.writeln("  Roon Commands:".to_string());
            for cmd_name in &roon_cmds {
                if let Some(cmd) = definitions.iter().find(|c| c.name == *cmd_name) {
                    let usage = cmd.usage.map(|u| format!(" {}", u)).unwrap_or_default();
                    out.writeln(format!("    {:<18}{} - {}", cmd.name.to_string() + &usage, "", cmd.description));
                }
            }
            out.writeln("".to_string());

            // UPnP Commands
            out.writeln("  UPnP Commands:".to_string());
            for cmd in upnp_cmds {
                let usage = cmd.usage.map(|u| format!(" {}", u)).unwrap_or_default();
                out.writeln(format!("    {:<18}{} - {}", cmd.name.to_string() + &usage, "", cmd.description));
            }
            out.writeln("".to_string());

            // dCS API Commands
            out.writeln("  dCS API Commands:".to_string());
            for cmd in dcs_cmds {
                let usage = cmd.usage.map(|u| format!(" {}", u)).unwrap_or_default();
                out.writeln(format!("    {:<18}{} - {}", cmd.name.to_string() + &usage, "", cmd.description));
            }
            out.writeln("".to_string());

            // General Commands
            out.writeln("  General:".to_string());
            for cmd_name in &general_cmds {
                if let Some(cmd) = definitions.iter().find(|c| c.name == *cmd_name) {
                    let usage = cmd.usage.map(|u| format!(" {}", u)).unwrap_or_default();
                    out.writeln(format!("    {:<18}{} - {}", cmd.name.to_string() + &usage, "", cmd.description));
                }
            }
            out.writeln("".to_string());
            Ok(())
        }
        "" => Ok(()),
        _ => {
            // Check if it's a UPnP or dCS command with optional arguments
            let parts: Vec<&str> = query_type.split_whitespace().collect();

            if parts.len() >= 1 {
                let command = parts[0];
                let arg = if parts.len() > 1 {
                    parts[1..].join(" ")
                } else {
                    String::new()
                };

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
                    "upnp-playing" => {
                        out.writeln("".to_string());
                        out.writeln("  Getting now playing information...".to_string());
                        out.writeln("".to_string());

                        // Get both transport state and position info
                        let transport_result = upnp::get_transport_info(&arg).await;
                        let position_result = upnp::get_position_info(&arg).await;

                        match (transport_result, position_result) {
                            (Ok(transport), Ok(position)) => {
                                // Show transport state
                                out.writeln(format!("  Status: {} ({})", transport.current_transport_state, transport.current_transport_status));
                                out.writeln("".to_string());

                                // Parse and display comprehensive track info
                                if let Some(track_info) = upnp::parse_track_info(&position.track_metadata) {
                                    // Track metadata
                                    if let Some(title) = &track_info.title {
                                        out.writeln(format!("  Title:  {}", title));
                                    }
                                    if let Some(artist) = &track_info.artist {
                                        out.writeln(format!("  Artist: {}", artist));
                                    }
                                    if let Some(album) = &track_info.album {
                                        out.writeln(format!("  Album:  {}", album));
                                    }
                                    if let Some(album_artist) = &track_info.album_artist {
                                        out.writeln(format!("  Album Artist: {}", album_artist));
                                    }
                                    out.writeln("".to_string());

                                    // Position information
                                    out.writeln("  Playback Position:".to_string());
                                    out.writeln(format!("    Position: {} / {}", position.rel_time, position.track_duration));
                                    out.writeln("".to_string());

                                    // Audio format
                                    let fmt = &track_info.audio_format;
                                    if fmt.sample_rate.is_some() || fmt.bits_per_sample.is_some() {
                                        out.writeln("  Audio Format:".to_string());
                                        if let Some(sr) = &fmt.sample_rate {
                                            out.writeln(format!("    Sample Rate: {} Hz", sr));
                                        }
                                        if let Some(bits) = &fmt.bits_per_sample {
                                            out.writeln(format!("    Bit Depth:   {} bits", bits));
                                        }
                                        if let Some(ch) = &fmt.channels {
                                            out.writeln(format!("    Channels:    {}", ch));
                                        }
                                        if let Some(br) = &fmt.bitrate {
                                            out.writeln(format!("    Bitrate:     {} bps", br));
                                        }
                                        if let Some(proto) = &fmt.protocol_info {
                                            out.writeln(format!("    Protocol:    {}", proto));
                                        }
                                        out.writeln("".to_string());
                                    }
                                } else {
                                    out.writeln("  No track metadata available".to_string());
                                    out.writeln("".to_string());
                                }
                                return Ok(());
                            }
                            (Err(e), _) => return Err(format!("Failed to get transport state: {}", e)),
                            (_, Err(e)) => return Err(format!("Failed to get position info: {}", e)),
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
                    "dcs-playing" => {
                        // Get dCS playback information
                        // Usage: dcs-playing [host]
                        // Example: dcs-playing dcs-vivaldi.local
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting playback info from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_playback_info(host).await {
                            Ok(info) => {
                                // Show playback state
                                if let Some(state) = &info.state {
                                    out.writeln(format!("  State: {}", state));
                                    out.writeln("".to_string());
                                }

                                // Show track metadata
                                if let Some(title) = &info.title {
                                    out.writeln(format!("  Title:  {}", title));
                                }
                                if let Some(artist) = &info.artist {
                                    out.writeln(format!("  Artist: {}", artist));
                                }
                                if let Some(album) = &info.album {
                                    out.writeln(format!("  Album:  {}", album));
                                }
                                if let Some(service) = &info.service_id {
                                    out.writeln(format!("  Source: {}", service));
                                }
                                out.writeln("".to_string());

                                // Show audio format
                                if let Some(format) = &info.audio_format {
                                    out.writeln("  Audio Format:".to_string());
                                    if let Some(sr) = format.sample_frequency {
                                        out.writeln(format!("    Sample Rate: {} Hz ({} kHz)", sr, sr / 1000));
                                    }
                                    if let Some(bits) = format.bits_per_sample {
                                        out.writeln(format!("    Bit Depth:   {} bits", bits));
                                    }
                                    if let Some(ch) = format.nr_audio_channels {
                                        out.writeln(format!("    Channels:    {}", ch));
                                    }
                                    out.writeln("".to_string());
                                }

                                // Show duration
                                if let Some(duration) = info.duration {
                                    let mins = duration / 60000;
                                    let secs = (duration % 60000) / 1000;
                                    out.writeln(format!("  Duration: {}:{:02}", mins, secs));
                                    out.writeln("".to_string());
                                }

                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get playback info: {}", e))
                        }
                    }
                    "dcs-format" => {
                        // Get dCS audio format information
                        // Usage: dcs-format [host]
                        // Example: dcs-format dcs-vivaldi.local
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting audio format from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_audio_format(host).await {
                            Ok(format) => {
                                out.writeln("  Current Audio Format:".to_string());
                                out.writeln("".to_string());

                                if let Some(sr) = format.sample_rate {
                                    out.writeln(format!("    Sample Rate: {} Hz ({} kHz)", sr, sr / 1000));
                                }
                                if let Some(bits) = format.bit_depth {
                                    out.writeln(format!("    Bit Depth:   {} bits", bits));
                                }
                                if let Some(mode) = &format.input_mode {
                                    out.writeln(format!("    Input Mode:  {}", mode));
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get audio format: {}", e))
                        }
                    }
                    "dcs-settings" => {
                        // Get dCS device settings
                        // Usage: dcs-settings [host]
                        // Example: dcs-settings dcs-vivaldi.local
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting device settings from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_device_settings(host).await {
                            Ok(settings) => {
                                out.writeln("  Device Settings:".to_string());
                                out.writeln("".to_string());

                                if let Some(brightness) = settings.display_brightness {
                                    out.writeln(format!("    Display Brightness: {}", brightness));
                                }
                                if let Some(off) = settings.display_off {
                                    out.writeln(format!("    Display Off: {}", off));
                                }
                                if let Some(sync) = &settings.sync_mode {
                                    out.writeln(format!("    Sync Mode: {}", sync));
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get device settings: {}", e))
                        }
                    }
                    "dcs-upsampler" => {
                        // Get dCS upsampler settings
                        // Usage: dcs-upsampler [host]
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting upsampler settings from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_upsampler_settings(host).await {
                            Ok(settings) => {
                                out.writeln("  Upsampler Settings:".to_string());
                                out.writeln("".to_string());

                                if let Some(rate) = settings.output_sample_rate {
                                    // Format the sample rate nicely
                                    let formatted = if rate >= 2822400 {
                                        // DSD rates
                                        let dsd_multiple = rate / 2822400;
                                        format!("{} Hz (DSD{} / {:.4} MHz)", rate, dsd_multiple * 64, rate as f64 / 1_000_000.0)
                                    } else if rate >= 1000000 {
                                        format!("{} Hz ({:.2} MHz)", rate, rate as f64 / 1_000_000.0)
                                    } else if rate >= 1000 {
                                        format!("{} Hz ({} kHz)", rate, rate / 1000)
                                    } else {
                                        format!("{} Hz", rate)
                                    };
                                    out.writeln(format!("    Output Rate: {}", formatted));
                                }
                                if let Some(filter) = settings.filter {
                                    out.writeln(format!("    Filter: {}", filter));
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get upsampler settings: {}", e))
                        }
                    }
                    "dcs-inputs" => {
                        // Get dCS digital inputs
                        // Usage: dcs-inputs [host]
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting digital inputs from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_input_info(host).await {
                            Ok(info) => {
                                if let Some(current) = &info.current_input {
                                    out.writeln(format!("  Current Input: {}", current));
                                    out.writeln("".to_string());
                                }

                                out.writeln("  Available Inputs:".to_string());
                                for input in &info.available_inputs {
                                    let marker = if Some(input) == info.current_input.as_ref() { " *" } else { "" };
                                    out.writeln(format!("    - {}{}", input, marker));
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get input info: {}", e))
                        }
                    }
                    "dcs-playmode" => {
                        // Get dCS play mode
                        // Usage: dcs-playmode [host]
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        let host = if parts.len() >= 2 {
                            parts[1]
                        } else {
                            DEFAULT_DCS_HOST
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Getting play mode from {}...", host));
                        out.writeln("".to_string());

                        match dcs::get_play_mode(host).await {
                            Ok(mode_info) => {
                                if let Some(mode) = &mode_info.mode {
                                    out.writeln(format!("  Play Mode: {}", mode));
                                } else {
                                    out.writeln("  Play Mode: (unknown)".to_string());
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get play mode: {}", e))
                        }
                    }
                    "dcs-menu" => {
                        // Browse dCS menu hierarchy
                        // Usage: dcs-menu [host] <path>
                        let parts: Vec<&str> = query_type.splitn(3, ' ').collect();

                        if parts.len() < 2 {
                            return Err("Usage: dcs-menu [host] <path>\n\nExamples:\n  dcs-menu dcsUiMenu:/ui/settings/audio\n  dcs-menu dcs-vivaldi.local dcsUiMenu:/ui/settings/audio\n  dcs-menu 192.168.50.31 dcsUiMenu:/ui/settings/frontPanel".to_string());
                        }

                        let (host, path) = if parts.len() >= 3 {
                            (parts[1], parts[2])
                        } else {
                            (DEFAULT_DCS_HOST, parts[1])
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Browsing menu: {}...", path));
                        out.writeln("".to_string());

                        match dcs::get_menu(host, path).await {
                            Ok(menu) => {
                                out.writeln(format!("  Menu: {}", menu.title));
                                out.writeln(format!("  Path: {}", menu.path));
                                out.writeln("".to_string());

                                if menu.items.is_empty() {
                                    out.writeln("  (No items)".to_string());
                                } else {
                                    out.writeln("  Items:".to_string());
                                    for (idx, item) in menu.items.iter().enumerate() {
                                        let type_marker = match item.item_type.as_str() {
                                            "container" => " →",
                                            "value" => "",
                                            _ => &format!(" ({})", item.item_type),
                                        };

                                        if let Some(ref value) = item.value {
                                            // Format value based on type
                                            let value_str = if let Some(i32_val) = value.get("i32_").and_then(|v| v.as_i64()) {
                                                format!(": {}", i32_val)
                                            } else if let Some(str_val) = value.get("string_").and_then(|v| v.as_str()) {
                                                format!(": {}", str_val)
                                            } else if let Some(bool_val) = value.get("bool_").and_then(|v| v.as_bool()) {
                                                format!(": {}", bool_val)
                                            } else {
                                                String::new()
                                            };
                                            out.writeln(format!("    {}. {}{}{}", idx + 1, item.title, type_marker, value_str));
                                        } else {
                                            out.writeln(format!("    {}. {}{}", idx + 1, item.title, type_marker));
                                        }
                                    }
                                }

                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to browse menu: {}", e))
                        }
                    }
                    "dcs-set-brightness" => {
                        // Set dCS display brightness
                        // Usage: dcs-set-brightness [host] <0-15>
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        if parts.len() < 2 {
                            return Err("Usage: dcs-set-brightness [host] <0-15>\n\nExamples:\n  dcs-set-brightness 10\n  dcs-set-brightness dcs-vivaldi.local 10\n  dcs-set-brightness 192.168.50.31 5".to_string());
                        }

                        let (host, brightness_str) = if parts.len() >= 3 {
                            (parts[1], parts[2])
                        } else {
                            (DEFAULT_DCS_HOST, parts[1])
                        };

                        // Parse brightness value
                        let brightness: i32 = brightness_str.parse()
                            .map_err(|_| format!("Invalid brightness value '{}'. Must be a number between 0 and 15.", brightness_str))?;

                        out.writeln("".to_string());
                        out.writeln(format!("  Setting display brightness to {}...", brightness));
                        out.writeln("".to_string());

                        match dcs::set_display_brightness(host, brightness).await {
                            Ok(_) => {
                                out.writeln("  ✓ Display brightness updated successfully".to_string());
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to set brightness: {}", e))
                        }
                    }
                    "dcs-set-display" => {
                        // Set dCS display on/off
                        // Usage: dcs-set-display [host] <on|off>
                        let parts: Vec<&str> = query_type.split_whitespace().collect();

                        if parts.len() < 2 {
                            return Err("Usage: dcs-set-display [host] <on|off>\n\nExamples:\n  dcs-set-display off\n  dcs-set-display dcs-vivaldi.local off\n  dcs-set-display 192.168.50.31 on".to_string());
                        }

                        let (host, state_str) = if parts.len() >= 3 {
                            (parts[1], parts[2].to_lowercase())
                        } else {
                            (DEFAULT_DCS_HOST, parts[1].to_lowercase())
                        };

                        // Parse on/off state
                        let display_off = match state_str.as_str() {
                            "off" => true,
                            "on" => false,
                            _ => return Err(format!("Invalid display state '{}'. Must be 'on' or 'off'.", state_str))
                        };

                        out.writeln("".to_string());
                        out.writeln(format!("  Turning display {}...", if display_off { "off" } else { "on" }));
                        out.writeln("".to_string());

                        match dcs::set_display_off(host, display_off).await {
                            Ok(_) => {
                                out.writeln(format!("  ✓ Display turned {} successfully", if display_off { "off" } else { "on" }));
                                out.writeln("".to_string());
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to set display: {}", e))
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

                // Handle reconnect command specially (needs mutable access)
                if command == "reconnect" {
                    if let Some(ref client) = client {
                        println!();
                        println!("  Reconnecting to Roon Core...");

                        let mut client_guard = client.lock().await;
                        match client_guard.reconnect().await {
                            Ok(_) => {
                                println!("  Successfully reconnected");
                                if let Some(name) = client_guard.get_core_name().await {
                                    println!("  Core: {}", name);
                                }
                            }
                            Err(e) => {
                                println!("  Reconnection failed: {}", e);
                            }
                        }
                        println!();
                    } else {
                        println!();
                        println!("  {} Roon commands require connection. Remove --upnp-only flag.", "Error:".bold().red());
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
        "upnp-playing".to_string(),
        // dCS API commands
        "dcs-playing".to_string(),
        "dcs-format".to_string(),
        "dcs-settings".to_string(),
        "dcs-upsampler".to_string(),
        "dcs-inputs".to_string(),
        "dcs-playmode".to_string(),
        "dcs-menu".to_string(),
        "dcs-set-brightness".to_string(),
        "dcs-set-display".to_string(),
    ];

    if client.is_some() {
        // Roon commands
        commands.extend(vec![
            "status".to_string(),
            "reconnect".to_string(),
            "zones".to_string(),
            "now-playing".to_string(),
            "queue".to_string(),
            "play".to_string(),
            "pause".to_string(),
            "stop".to_string(),
            "mute".to_string(),
        ]);
    }
    commands.sort();

    // Get WebSocket receiver if we have a client
    let ws_rx = if let Some(ref client) = client {
        Some(client.lock().await.subscribe_ws())
    } else {
        None
    };

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

            // Handle reconnect command (needs mutable access)
            if command.trim() == "reconnect" {
                if let Some(ref client) = client_clone {
                    let mut buffer = buffer_for_commands.lock().unwrap();
                    buffer.push("".to_string());
                    buffer.push("  Reconnecting to Roon Core...".to_string());
                    drop(buffer); // Release lock before async operation

                    let mut client_guard = client.lock().await;
                    match client_guard.reconnect().await {
                        Ok(_) => {
                            let mut buffer = buffer_for_commands.lock().unwrap();
                            buffer.push("  Successfully reconnected".to_string());
                            if let Some(name) = client_guard.get_core_name().await {
                                buffer.push(format!("  Core: {}", name));
                            }
                            buffer.push("".to_string());
                        }
                        Err(e) => {
                            let mut buffer = buffer_for_commands.lock().unwrap();
                            buffer.push(format!("  Reconnection failed: {}", e));
                            buffer.push("".to_string());
                        }
                    }
                } else {
                    let mut buffer = buffer_for_commands.lock().unwrap();
                    buffer.push("".to_string());
                    buffer.push("  Error: Roon commands require connection. Remove --upnp-only flag.".to_string());
                    buffer.push("".to_string());
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
    }, exit_flag, commands, ws_rx).await?;

    Ok(())
}
