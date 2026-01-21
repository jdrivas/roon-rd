use std::sync::Arc;
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
            "upnp-position".to_string(),
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

/// Execute a query command against the Roon client (or UPnP-only commands)
async fn execute_query(client: Option<&RoonClient>, query_type: &str, _verbose: bool) -> Result<(), String> {
    // Note: Log level is now managed by the verbose command in interactive mode
    // The verbose parameter is kept for compatibility but not used here

    match query_type {
        "status" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let connected = client.is_connected().await;
            println!();
            if connected {
                let core_name = client.get_core_name().await;
                println!("  Status: Connected");
                if let Some(name) = core_name {
                    println!("  Core:   {}", name);
                }
            } else {
                println!("  Status: Not connected");
                println!();
                println!("  Authorize the extension in Roon Settings > Extensions");
            }
            println!();
            Ok(())
        }
        "zones" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let zones = client.get_zones().await;

            if zones.is_empty() {
                println!();
                println!("  No zones found.");
                if !client.is_connected().await {
                    println!("  Not connected to Roon Core. Please authorize the extension.");
                } else {
                    println!("  Connected but no active zones.");
                }
                println!();
            } else {
                println!();
                for zone in &zones {
                    // Zone name with state
                    let state_str = format!("{:?}", zone.state).to_lowercase();
                    println!("  {} ({})", zone.display_name, state_str);
                    println!("    ID: {}", zone.zone_id);

                    // Show outputs (devices in this zone) indented
                    for output in &zone.outputs {
                        if output.display_name != zone.display_name {
                            println!("    └─ {}", output.display_name);
                        }
                    }
                }
                println!();
            }
            Ok(())
        }
        "now-playing" => {
            let client = client.ok_or("Roon commands require connection. Remove --upnp-only flag.")?;
            let zones = client.get_zones().await;

            if zones.is_empty() {
                println!();
                println!("  No zones found.");
                if !client.is_connected().await {
                    println!("  Not connected to Roon Core. Please authorize the extension.");
                }
                println!();
            } else {
                let mut playing_count = 0;

                println!();
                for zone in &zones {
                    if let Some(now_playing) = &zone.now_playing {
                        playing_count += 1;

                        let state_str = format!("{:?}", zone.state).to_lowercase();
                        let three_line = &now_playing.three_line;

                        // Zone header
                        println!("  {} ({})", zone.display_name, state_str);
                        println!("  ─────────────────────────────────────");

                        // Track info
                        println!("    {}", three_line.line1);
                        if !three_line.line2.is_empty() {
                            println!("    {}", three_line.line2);
                        }
                        if !three_line.line3.is_empty() {
                            println!("    {}", three_line.line3);
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

                            println!();
                            println!("    {} {} / {}", bar, format_duration(position), format_duration(length));
                        }

                        println!();
                    }
                }

                if playing_count == 0 {
                    println!("  No zones are currently playing.");
                    println!();
                }
            }
            Ok(())
        }
        "upnp-discover" => {
            println!();
            println!("  Discovering UPnP devices (5 second timeout)...");
            println!();

            match upnp::discover_devices(5).await {
                Ok(devices) => {
                    if devices.is_empty() {
                        println!("  No UPnP devices found.");
                    } else {
                        println!("  Found {} device(s):", devices.len());
                        for (i, device) in devices.iter().enumerate() {
                            println!();
                            println!("  Device {}:", i + 1);
                            println!("    Location: {}", device.location);
                            if let Some(device_type) = &device.device_type {
                                println!("    Type: {}", device_type);
                            }

                            // Try to get detailed info
                            if let Ok(info) = upnp::get_device_info(&device.location).await {
                                println!("    Name: {}", info.friendly_name);
                                if let Some(mfr) = info.manufacturer {
                                    println!("    Manufacturer: {}", mfr);
                                }
                                if let Some(model) = info.model_name {
                                    println!("    Model: {}", model);
                                }
                            }
                        }
                    }
                    println!();
                    Ok(())
                }
                Err(e) => Err(format!("Discovery failed: {}", e))
            }
        }
        "upnp-renderers" => {
            println!();
            println!("  Discovering UPnP MediaRenderers (5 second timeout)...");
            println!();

            match upnp::discover_media_renderers(5).await {
                Ok(devices) => {
                    if devices.is_empty() {
                        println!("  No MediaRenderer devices found.");
                    } else {
                        println!("  Found {} MediaRenderer(s):", devices.len());
                        for (i, device) in devices.iter().enumerate() {
                            println!();
                            println!("  Renderer {}:", i + 1);
                            println!("    Location: {}", device.location);

                            // Try to get detailed info
                            if let Ok(info) = upnp::get_device_info(&device.location).await {
                                println!("    Name: {}", info.friendly_name);
                                if let Some(mfr) = info.manufacturer {
                                    println!("    Manufacturer: {}", mfr);
                                }
                                if let Some(model) = info.model_name {
                                    println!("    Model: {}", model);
                                }
                            }
                        }
                    }
                    println!();
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
            println!();
            println!("  roon-rd version {}", env!("CARGO_PKG_VERSION"));
            println!();
            Ok(())
        }
        "help" => {
            println!();
            println!("  Available commands:");
            println!();
            println!("  Roon Commands:");
            println!("    status             - Show connection status");
            println!("    zones              - List available zones");
            println!("    now-playing        - Show currently playing tracks");
            println!("    play <zone_id>     - Start playback in zone");
            println!("    pause <zone_id>    - Pause playback in zone");
            println!("    stop <zone_id>     - Stop playback in zone");
            println!("    mute <zone_id>     - Toggle mute for zone");
            println!();
            println!("  UPnP Commands:");
            println!("    upnp-discover      - Discover all UPnP devices on network");
            println!("    upnp-renderers     - Discover UPnP MediaRenderer devices");
            println!("    upnp-info <url>    - Get detailed device information");
            println!("    upnp-position <url> - Get current playback position and metadata");
            println!();
            println!("  General:");
            println!("    verbose            - Toggle verbose logging on/off");
            println!("    version            - Show version information");
            println!("    help               - Show this help message");
            println!("    quit               - Exit interactive mode");
            println!();
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
                        println!();
                        println!("  Getting device information...");
                        println!();

                        match upnp::get_device_info(&arg).await {
                            Ok(info) => {
                                println!("  Device Information:");
                                println!("    Name: {}", info.friendly_name);
                                println!("    Type: {}", info.device_type);
                                if let Some(mfr) = info.manufacturer {
                                    println!("    Manufacturer: {}", mfr);
                                }
                                if let Some(model) = info.model_name {
                                    println!("    Model: {}", model);
                                }
                                if let Some(model_num) = info.model_number {
                                    println!("    Model Number: {}", model_num);
                                }
                                if let Some(serial) = info.serial_number {
                                    println!("    Serial: {}", serial);
                                }
                                println!();
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get device info: {}", e))
                        }
                    }
                    "upnp-position" => {
                        println!();
                        println!("  Getting position info...");
                        println!();

                        match upnp::get_position_info(&arg).await {
                            Ok(info) => {
                                println!("  Position Information:");
                                println!("    Track: {}", info.track);
                                println!("    Duration: {}", info.track_duration);
                                println!("    Position: {}", info.rel_time);
                                println!("    URI: {}", info.track_uri);
                                println!();

                                // Try to parse audio format from metadata
                                if !info.track_metadata.is_empty() {
                                    println!("  Track Metadata (DIDL-Lite):");
                                    println!("    {}", info.track_metadata.chars().take(200).collect::<String>());
                                    if info.track_metadata.len() > 200 {
                                        println!("    ... (truncated)");
                                    }
                                    println!();

                                    if let Some(format) = upnp::parse_audio_format(&info.track_metadata) {
                                        println!("  Audio Format:");
                                        if let Some(sr) = format.sample_rate {
                                            println!("    Sample Rate: {} Hz", sr);
                                        }
                                        if let Some(bits) = format.bits_per_sample {
                                            println!("    Bit Depth: {} bits", bits);
                                        }
                                        if let Some(ch) = format.channels {
                                            println!("    Channels: {}", ch);
                                        }
                                        if let Some(br) = format.bitrate {
                                            println!("    Bitrate: {} bps", br);
                                        }
                                        println!();
                                    }
                                }
                                return Ok(());
                            }
                            Err(e) => return Err(format!("Failed to get position info: {}", e))
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
                                println!();
                                println!("  {} command sent to zone", command);
                                println!();
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
                                println!();
                                println!("  Mute toggled for zone");
                                println!();
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

    Ok(())
}
