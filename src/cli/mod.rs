use std::sync::Arc;
use tokio::sync::Mutex;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use crate::roon::RoonClient;

/// Format duration in mm:ss format
fn format_duration(seconds: u32) -> String {
    let mins = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", mins, secs)
}

/// Execute a query command against the Roon client
async fn execute_query(client: &RoonClient, query_type: &str) -> Result<(), String> {
    match query_type {
        "status" => {
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
        "help" => {
            println!();
            println!("  Available commands:");
            println!("    status             - Show connection status");
            println!("    zones              - List available zones");
            println!("    now-playing        - Show currently playing tracks");
            println!("    play <zone_id>     - Start playback in zone");
            println!("    pause <zone_id>    - Pause playback in zone");
            println!("    stop <zone_id>     - Stop playback in zone");
            println!("    help               - Show this help message");
            println!("    quit               - Exit interactive mode");
            println!();
            Ok(())
        }
        "" => Ok(()),
        _ => {
            // Check if it's a control command with zone_id
            let parts: Vec<&str> = query_type.split_whitespace().collect();
            if parts.len() == 2 {
                let command = parts[0];
                let zone_id = parts[1];

                match command {
                    "play" | "pause" | "stop" => {
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
                    _ => Err(format!("Unknown command: {}\nType 'help' for available commands.", query_type))
                }
            } else if parts.len() == 1 && (parts[0] == "play" || parts[0] == "pause" || parts[0] == "stop") {
                Err(format!("Usage: {} <zone_id>\nUse 'zones' to see available zone IDs.", parts[0]))
            } else {
                Err(format!("Unknown command: {}\nType 'help' for available commands.", query_type))
            }
        }
    }
}

/// Handle CLI query commands
pub async fn handle_query(client: Arc<Mutex<RoonClient>>, query_type: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Wait for authorization, checking every 15 seconds with status messages
    // No timeout - wait indefinitely until authorized
    {
        let client = client.lock().await;
        client.wait_for_authorization(15, None).await;

        // Give a brief moment for zone data to arrive after connection
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    let client = client.lock().await;
    execute_query(&client, query_type).await.map_err(|e| e.into())
}

/// Handle interactive mode - read commands from stdin with history support
pub async fn handle_interactive(client: Arc<Mutex<RoonClient>>) -> Result<(), Box<dyn std::error::Error>> {
    println!();
    println!("Roon Remote Display - Interactive Mode");
    println!("Type 'help' for available commands, 'quit' to exit.");
    println!("Use arrow keys or Ctrl-P/Ctrl-N to navigate command history.");
    println!();
    println!("Please enable the extension in Roon Settings > Extensions.");
    println!();

    // Wait for authorization
    {
        let client = client.lock().await;
        client.wait_for_authorization(15, None).await;

        // Give a brief moment for zone data to arrive after connection
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // Create readline editor with history support
    let mut rl = DefaultEditor::new()?;

    loop {
        // Read line with history support
        let readline = rl.readline("> ");

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

                // Execute the command
                let client = client.lock().await;
                if let Err(e) = execute_query(&client, command).await {
                    println!("  Error: {}", e);
                    println!();
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
