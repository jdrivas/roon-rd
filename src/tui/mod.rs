use std::io::{self, stdout, BufRead, BufReader, Write};
use std::sync::{Arc, Mutex as StdMutex};
use std::path::PathBuf;
use std::fs;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
    Terminal,
};
use log::{Level, Metadata, Record};
use simplelog::{Config as LogConfig, LevelFilter, SharedLogger};

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

/// Shared message buffer that can be accessed from multiple threads
pub struct MessageBuffer {
    messages: Vec<String>,
    max_messages: usize,
}

impl MessageBuffer {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
        }
    }

    pub fn push(&mut self, message: String) {
        self.messages.push(message);
        if self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

/// Shared event buffer for Roon events
pub struct EventBuffer {
    events: Vec<String>,
    max_events: usize,
}

impl EventBuffer {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: Vec::new(),
            max_events,
        }
    }

    pub fn push(&mut self, event: String) {
        self.events.push(event);
        if self.events.len() > self.max_events {
            self.events.remove(0);
        }
    }

    pub fn events(&self) -> &[String] {
        &self.events
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

/// Zone display information
#[derive(Clone, Debug)]
pub struct ZoneDisplay {
    pub zone_id: String,
    pub zone_name: String,
    pub state: String,
    pub track: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub format: Option<String>,
    pub position_seconds: Option<i64>,
    pub length_seconds: Option<u32>,
}

/// Zone display buffer
pub struct ZoneBuffer {
    zones: Vec<ZoneDisplay>,
}

impl ZoneBuffer {
    pub fn new() -> Self {
        Self {
            zones: Vec::new(),
        }
    }

    pub fn update(&mut self, zones: Vec<ZoneDisplay>) {
        self.zones = zones;
    }

    pub fn update_position(&mut self, zone_id: &str, position_seconds: Option<i64>) {
        if let Some(zone) = self.zones.iter_mut().find(|z| z.zone_id == zone_id) {
            zone.position_seconds = position_seconds;
        }
    }

    pub fn zones(&self) -> &[ZoneDisplay] {
        &self.zones
    }
}

/// Format seconds as MM:SS or HH:MM:SS if >= 1 hour
fn format_time(seconds: i64) -> String {
    let abs_seconds = seconds.abs();
    let hours = abs_seconds / 3600;
    let minutes = (abs_seconds % 3600) / 60;
    let secs = abs_seconds % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

/// Get priority for zone state sorting (lower = higher priority)
fn get_state_priority(state: &str) -> u8 {
    match state {
        "playing" => 0,
        "loading" => 1,
        "paused" => 2,
        "stopped" => 3,
        _ => 4,
    }
}

/// Custom logger that writes to the TUI message buffer
pub struct TuiLogger {
    buffer: Arc<StdMutex<MessageBuffer>>,
    level: Arc<StdMutex<LevelFilter>>,
    config: LogConfig,
}

impl TuiLogger {
    pub fn new(buffer: Arc<StdMutex<MessageBuffer>>, level: Arc<StdMutex<LevelFilter>>, config: LogConfig) -> Self {
        Self {
            buffer,
            level,
            config,
        }
    }
}

impl log::Log for TuiLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if let Ok(level) = self.level.lock() {
            metadata.level() <= *level
        } else {
            false
        }
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // Format log message similar to simplelog
            let level_string = match record.level() {
                Level::Error => "ERROR",
                Level::Warn => "WARN",
                Level::Info => "INFO",
                Level::Debug => "DEBUG",
                Level::Trace => "TRACE",
            };

            let message = format!(
                "[{:5}] {}",
                level_string,
                record.args()
            );

            if let Ok(mut buffer) = self.buffer.lock() {
                buffer.push(message);
            }
        }
    }

    fn flush(&self) {}
}

impl SharedLogger for TuiLogger {
    fn level(&self) -> LevelFilter {
        if let Ok(level) = self.level.lock() {
            *level
        } else {
            LevelFilter::Off
        }
    }

    fn config(&self) -> Option<&LogConfig> {
        Some(&self.config)
    }

    fn as_log(self: Box<Self>) -> Box<dyn log::Log> {
        Box::new(*self)
    }
}

/// Which window is currently maximized
#[derive(Debug, Clone, Copy, PartialEq)]
enum MaximizedWindow {
    None,
    Zones,
    Events,
    Output,
}

/// Terminal UI application state
pub struct App {
    /// Current input string
    input: String,
    /// Cursor position in the input
    cursor_position: usize,
    /// Message buffer (shared with logger)
    message_buffer: Arc<StdMutex<MessageBuffer>>,
    /// Zone buffer for current zone states
    zone_buffer: Option<Arc<StdMutex<ZoneBuffer>>>,
    /// Event buffer for Roon events
    event_buffer: Option<Arc<StdMutex<EventBuffer>>>,
    /// Command history
    history: Vec<String>,
    /// Current position in history (None = not browsing)
    history_position: Option<usize>,
    /// Whether the app should exit
    should_exit: bool,
    /// Prompt callback to get current prompt
    prompt_fn: Arc<StdMutex<Box<dyn Fn() -> String + Send>>>,
    /// Scroll offset from the bottom (0 = at bottom/auto-scroll, >0 = scrolled up)
    scroll_offset: usize,
    /// Scrollbar state for output
    scrollbar_state: ScrollbarState,
    /// Scrollbar state for events
    event_scrollbar_state: ScrollbarState,
    /// Event scroll offset
    event_scroll_offset: usize,
    /// Available commands for completion
    commands: Vec<String>,
    /// Current completion candidates
    completion_candidates: Vec<String>,
    /// Current position in completion candidates
    completion_index: Option<usize>,
    /// Which window is currently maximized
    maximized_window: MaximizedWindow,
}

impl App {
    pub fn new<F>(message_buffer: Arc<StdMutex<MessageBuffer>>, prompt_fn: F, commands: Vec<String>) -> Self
    where
        F: Fn() -> String + Send + 'static,
    {
        // Load history from file
        let history = load_history();

        Self {
            input: String::new(),
            cursor_position: 0,
            message_buffer,
            zone_buffer: None,
            event_buffer: None,
            history,
            history_position: None,
            should_exit: false,
            prompt_fn: Arc::new(StdMutex::new(Box::new(prompt_fn))),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            event_scrollbar_state: ScrollbarState::default(),
            event_scroll_offset: 0,
            commands,
            completion_candidates: Vec::new(),
            completion_index: None,
            maximized_window: MaximizedWindow::None,
        }
    }

    pub fn set_zone_buffer(&mut self, zone_buffer: Arc<StdMutex<ZoneBuffer>>) {
        self.zone_buffer = Some(zone_buffer);
    }

    pub fn set_event_buffer(&mut self, event_buffer: Arc<StdMutex<EventBuffer>>) {
        self.event_buffer = Some(event_buffer);
    }

    fn get_prompt(&self) -> String {
        if let Ok(prompt_fn) = self.prompt_fn.lock() {
            prompt_fn()
        } else {
            "> ".to_string()
        }
    }

    /// Complete current input with matching commands
    fn complete(&mut self) {
        // Get the current word (first word in input)
        let input_trimmed = self.input.trim_start();

        // If we're not already in a completion cycle, find candidates
        if self.completion_index.is_none() {
            self.completion_candidates.clear();

            if input_trimmed.is_empty() {
                // No input - show all commands
                self.completion_candidates = self.commands.clone();
            } else {
                // Find matching commands
                for cmd in &self.commands {
                    if cmd.starts_with(input_trimmed) {
                        self.completion_candidates.push(cmd.clone());
                    }
                }
            }

            self.completion_candidates.sort();

            if self.completion_candidates.is_empty() {
                return;
            }

            // Start at first candidate
            self.completion_index = Some(0);
            self.input = self.completion_candidates[0].clone();
            self.cursor_position = self.input.len();
        } else {
            // Cycle to next candidate
            if let Some(idx) = self.completion_index {
                let next_idx = (idx + 1) % self.completion_candidates.len();
                self.completion_index = Some(next_idx);
                self.input = self.completion_candidates[next_idx].clone();
                self.cursor_position = self.input.len();
            }
        }
    }

    /// Reset completion state
    fn reset_completion(&mut self) {
        self.completion_candidates.clear();
        self.completion_index = None;
    }

    /// Handle keyboard input
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        match (key.code, key.modifiers) {
            // ESC - restore normal layout if maximized
            (KeyCode::Esc, _) => {
                if self.maximized_window != MaximizedWindow::None {
                    self.maximized_window = MaximizedWindow::None;
                }
                self.reset_completion();
                None
            }
            // Ctrl+C or Ctrl+D - exit
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.should_exit = true;
                self.reset_completion();
                None
            }
            // Ctrl+P - previous command in history (emacs style)
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                if !self.history.is_empty() {
                    let new_pos = match self.history_position {
                        None => Some(self.history.len() - 1),
                        Some(0) => Some(0),
                        Some(pos) => Some(pos - 1),
                    };
                    if let Some(pos) = new_pos {
                        self.input = self.history[pos].clone();
                        self.cursor_position = self.input.len();
                        self.history_position = new_pos;
                    }
                }
                self.reset_completion();
                None
            }
            // Ctrl+N - next command in history (emacs style)
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                if let Some(pos) = self.history_position {
                    if pos + 1 < self.history.len() {
                        let new_pos = pos + 1;
                        self.input = self.history[new_pos].clone();
                        self.cursor_position = self.input.len();
                        self.history_position = Some(new_pos);
                    } else {
                        // At end of history, clear input
                        self.input.clear();
                        self.cursor_position = 0;
                        self.history_position = None;
                    }
                }
                self.reset_completion();
                None
            }
            // Enter - submit command
            (KeyCode::Enter, _) => {
                let command = self.input.clone();
                if !command.is_empty() {
                    self.history.push(command.clone());
                    // Save history to file after each command
                    let _ = save_history(&self.history);
                }
                self.input.clear();
                self.cursor_position = 0;
                self.history_position = None;
                self.reset_completion();
                Some(command)
            }
            // Tab - command completion
            (KeyCode::Tab, _) => {
                self.complete();
                None
            }
            // Backspace - delete character before cursor
            (KeyCode::Backspace, _) => {
                if self.cursor_position > 0 {
                    self.input.remove(self.cursor_position - 1);
                    self.cursor_position -= 1;
                }
                self.reset_completion();
                None
            }
            // Delete - delete character at cursor
            (KeyCode::Delete, _) => {
                if self.cursor_position < self.input.len() {
                    self.input.remove(self.cursor_position);
                }
                self.reset_completion();
                None
            }
            // Left arrow - move cursor left
            (KeyCode::Left, _) => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                }
                self.reset_completion();
                None
            }
            // Right arrow - move cursor right
            (KeyCode::Right, _) => {
                if self.cursor_position < self.input.len() {
                    self.cursor_position += 1;
                }
                self.reset_completion();
                None
            }
            // Home - move cursor to start (when Ctrl is pressed, scroll to top)
            (KeyCode::Home, KeyModifiers::CONTROL) => {
                let messages = self.message_buffer.lock().unwrap();
                let total = messages.messages().len();
                if total > 0 {
                    self.scroll_offset = total.saturating_sub(1);
                }
                None
            }
            (KeyCode::Home, _) => {
                self.cursor_position = 0;
                None
            }
            // End - move cursor to end (when Ctrl is pressed, scroll to bottom)
            (KeyCode::End, KeyModifiers::CONTROL) => {
                self.scroll_offset = 0;
                None
            }
            (KeyCode::End, _) => {
                self.cursor_position = self.input.len();
                None
            }
            // Up arrow - scroll up in output (or browse history when Ctrl is pressed)
            (KeyCode::Up, KeyModifiers::CONTROL) => {
                if !self.history.is_empty() {
                    let new_pos = match self.history_position {
                        None => Some(self.history.len() - 1),
                        Some(0) => Some(0),
                        Some(pos) => Some(pos - 1),
                    };
                    if let Some(pos) = new_pos {
                        self.input = self.history[pos].clone();
                        self.cursor_position = self.input.len();
                        self.history_position = new_pos;
                    }
                }
                self.reset_completion();
                None
            }
            (KeyCode::Up, KeyModifiers::ALT) => {
                // Alt+Up: Scroll up in events
                if let Some(ref event_buffer) = self.event_buffer {
                    let events = event_buffer.lock().unwrap();
                    // Count total lines (split multi-line events)
                    let total_lines: usize = events.events().iter()
                        .map(|evt| evt.lines().count())
                        .sum();
                    if self.event_scroll_offset < total_lines.saturating_sub(1) {
                        self.event_scroll_offset += 1;
                    }
                }
                None
            }
            (KeyCode::Up, _) => {
                // Scroll up in output
                let messages = self.message_buffer.lock().unwrap();
                let total = messages.messages().len();
                if self.scroll_offset < total.saturating_sub(1) {
                    self.scroll_offset += 1;
                }
                None
            }
            // Down arrow - scroll down in output (or browse history when Ctrl is pressed)
            (KeyCode::Down, KeyModifiers::CONTROL) => {
                if let Some(pos) = self.history_position {
                    if pos + 1 < self.history.len() {
                        let new_pos = pos + 1;
                        self.input = self.history[new_pos].clone();
                        self.cursor_position = self.input.len();
                        self.history_position = Some(new_pos);
                    } else {
                        // At end of history, clear input
                        self.input.clear();
                        self.cursor_position = 0;
                        self.history_position = None;
                    }
                }
                self.reset_completion();
                None
            }
            (KeyCode::Down, KeyModifiers::ALT) => {
                // Alt+Down: Scroll down in events
                if self.event_scroll_offset > 0 {
                    self.event_scroll_offset -= 1;
                }
                None
            }
            (KeyCode::Down, _) => {
                // Scroll down in output
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                }
                None
            }
            // PageUp - scroll up one page
            (KeyCode::PageUp, _) => {
                let messages = self.message_buffer.lock().unwrap();
                let total = messages.messages().len();
                // Scroll up by 10 lines (approximation of a page)
                self.scroll_offset = (self.scroll_offset + 10).min(total.saturating_sub(1));
                None
            }
            // PageDown - scroll down one page
            (KeyCode::PageDown, _) => {
                // Scroll down by 10 lines
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                None
            }
            // Regular character input
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.input.insert(self.cursor_position, c);
                self.cursor_position += 1;
                self.reset_completion();
                None
            }
            _ => None,
        }
    }

    /// Handle mouse events
    pub fn handle_mouse(&mut self, mouse_event: event::MouseEvent, zone_area: Option<Rect>, event_area: Option<Rect>, output_area: Option<Rect>) {
        use event::MouseEventKind;

        // Only handle left clicks
        if mouse_event.kind != MouseEventKind::Down(event::MouseButton::Left) {
            return;
        }

        let row = mouse_event.row;
        let col = mouse_event.column;

        // Check if click is on zones window title
        if let Some(zone_rect) = zone_area {
            if row == zone_rect.y && col >= zone_rect.x && col < zone_rect.x + zone_rect.width {
                // Toggle maximized state
                self.maximized_window = if self.maximized_window == MaximizedWindow::Zones {
                    MaximizedWindow::None
                } else {
                    MaximizedWindow::Zones
                };
                return;
            }
        }

        // Check if click is on events window title (if events exist)
        if let Some(event_rect) = event_area {
            // Title is on top border (y position)
            if row == event_rect.y && col >= event_rect.x && col < event_rect.x + event_rect.width {
                // Toggle maximized state
                self.maximized_window = if self.maximized_window == MaximizedWindow::Events {
                    MaximizedWindow::None
                } else {
                    MaximizedWindow::Events
                };
                return;
            }
        }

        // Check if click is on output window title
        if let Some(output_rect) = output_area {
            if row == output_rect.y && col >= output_rect.x && col < output_rect.x + output_rect.width {
                // Toggle maximized state
                self.maximized_window = if self.maximized_window == MaximizedWindow::Output {
                    MaximizedWindow::None
                } else {
                    MaximizedWindow::Output
                };
                return;
            }
        }
    }

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    /// Render the UI
    /// Returns the zone, event and output area Rects for mouse click detection
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<(Option<Rect>, Option<Rect>, Option<Rect>)> {
        let mut zone_area: Option<Rect> = None;
        let mut event_area: Option<Rect> = None;
        let mut output_area: Option<Rect> = None;

        terminal.draw(|f| {
            // Determine if we have zones to display
            let has_zones = self.zone_buffer.is_some();
            let has_events = self.event_buffer.is_some();

            // Split terminal based on maximized state and available windows
            let chunks = match self.maximized_window {
                MaximizedWindow::Zones => {
                    // Zones maximized
                    Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Min(1),        // Zones area (takes all space)
                            Constraint::Length(3),     // Input area (fixed 3 lines)
                        ])
                        .split(f.area())
                }
                MaximizedWindow::Events => {
                    // Events maximized: events (almost full), input (bottom)
                    Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Min(1),        // Events area (takes all space)
                            Constraint::Length(3),     // Input area (fixed 3 lines)
                        ])
                        .split(f.area())
                }
                MaximizedWindow::Output => {
                    if self.event_buffer.is_some() {
                        // Output maximized: output (almost full), input (bottom)
                        // Events window hidden
                        Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Min(1),        // Output area (takes all space)
                                Constraint::Length(3),     // Input area (fixed 3 lines)
                            ])
                            .split(f.area())
                    } else {
                        // No events, same as normal 2-pane
                        Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Min(1),
                                Constraint::Length(3),
                            ])
                            .split(f.area())
                    }
                }
                MaximizedWindow::None => {
                    // Normal layout - varies based on what windows we have
                    match (has_zones, has_events) {
                        (true, true) => {
                            // 4-pane layout: zones, events, output, input
                            Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([
                                    Constraint::Length(5),       // Zones area (5 lines per zone + borders)
                                    Constraint::Percentage(25),  // Events area (25%)
                                    Constraint::Min(1),          // Output area (remaining)
                                    Constraint::Length(3),       // Input area
                                ])
                                .split(f.area())
                        }
                        (true, false) => {
                            // 3-pane layout: zones, output, input
                            Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([
                                    Constraint::Length(5),   // Zones area
                                    Constraint::Min(1),      // Output area
                                    Constraint::Length(3),   // Input area
                                ])
                                .split(f.area())
                        }
                        (false, true) => {
                            // 3-pane layout: events, output, input
                            Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([
                                    Constraint::Percentage(30),  // Events area
                                    Constraint::Min(1),          // Output area
                                    Constraint::Length(3),       // Input area
                                ])
                                .split(f.area())
                        }
                        (false, false) => {
                            // 2-pane layout: output, input
                            Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([
                                    Constraint::Min(1),      // Output area
                                    Constraint::Length(3),   // Input area
                                ])
                                .split(f.area())
                        }
                    }
                }
            };

            // Render zones window if available and not hidden
            if let Some(ref zone_buffer) = self.zone_buffer {
                if self.maximized_window != MaximizedWindow::Events && self.maximized_window != MaximizedWindow::Output {
                    let zones = zone_buffer.lock().unwrap();
                    let zone_list = zones.zones();

                    // Format zones as lines
                    let zone_lines: Vec<Line> = if zone_list.is_empty() {
                        vec![Line::from("No zones available")]
                    } else {
                        let zone_width = chunks[0].width.saturating_sub(2) as usize; // Account for borders

                        zone_list.iter().flat_map(|z| {
                            // Format the position counter
                            let position_str = if let (Some(pos), Some(len)) = (z.position_seconds, z.length_seconds) {
                                format!("{} / {}", format_time(pos), format_time(len as i64))
                            } else {
                                String::new()
                            };

                            // Create zone title line: <zone-name>      <state>      <position>
                            // The state should be centered at the window midpoint
                            let zone_title = format!("Zone: {}", z.zone_name);
                            let state_str = z.state.clone();

                            let title_line = if zone_width > 0 {
                                // Calculate where the middle of the state should be (at window midpoint)
                                let window_midpoint = zone_width / 2;
                                let state_len = state_str.len();
                                let state_midpoint = state_len / 2;

                                // Calculate where state should start to center it
                                let state_start = if window_midpoint >= state_midpoint {
                                    window_midpoint - state_midpoint
                                } else {
                                    0
                                };

                                // Build the line with state centered
                                let mut line = String::new();

                                // Add zone title on the left
                                line.push_str(&zone_title);

                                // Add spacing to position state at center
                                if state_start > zone_title.len() {
                                    let padding = state_start - zone_title.len();
                                    line.push_str(&" ".repeat(padding));
                                    line.push_str(&state_str);
                                } else {
                                    // Not enough room to center state after zone title, add minimal spacing
                                    line.push_str("  ");
                                    line.push_str(&state_str);
                                }

                                // Add position on the right if available
                                if !position_str.is_empty() {
                                    let current_len = line.len();
                                    if current_len + 2 + position_str.len() <= zone_width {
                                        // Calculate padding to right-justify position
                                        let padding = zone_width - current_len - position_str.len();
                                        line.push_str(&" ".repeat(padding));
                                        line.push_str(&position_str);
                                    } else if current_len + position_str.len() + 1 <= zone_width {
                                        // Minimal spacing
                                        line.push(' ');
                                        line.push_str(&position_str);
                                    }
                                }

                                line
                            } else {
                                zone_title
                            };

                            vec![
                                Line::from(title_line),
                                Line::from(format!("  Track: {}  |  Artist: {}  |  Album: {}  |  Format: {}",
                                    z.track.as_deref().unwrap_or("-"),
                                    z.artist.as_deref().unwrap_or("-"),
                                    z.album.as_deref().unwrap_or("-"),
                                    z.format.as_deref().unwrap_or("-")
                                )),
                            ]
                        }).collect()
                    };

                    let zones_widget = Paragraph::new(zone_lines)
                        .block(Block::default().borders(Borders::ALL).title("Zones"))
                        .style(Style::default().fg(Color::Cyan))
                        .wrap(Wrap { trim: false });
                    f.render_widget(zones_widget, chunks[0]);

                    // Store zone area for mouse click detection
                    zone_area = Some(chunks[0]);
                }
            }

            // Render events area if available and not hidden (when output or zones maximized)
            if let Some(ref event_buffer) = self.event_buffer {
                if self.maximized_window != MaximizedWindow::Output && self.maximized_window != MaximizedWindow::Zones {
                let events = event_buffer.lock().unwrap();
                let all_events = events.events();

                // Determine events chunk index based on layout
                let events_chunk_idx = match self.maximized_window {
                    MaximizedWindow::Events => 0,
                    _ => if has_zones { 1 } else { 0 }
                };

                let available_height = chunks[events_chunk_idx].height.saturating_sub(2) as usize;

                // Split events by newlines to handle multi-line debug output
                let all_event_lines: Vec<String> = all_events
                    .iter()
                    .flat_map(|evt| evt.lines().map(|line| line.to_string()))
                    .collect();
                let total_event_lines = all_event_lines.len();

                let (visible_events, event_scroll_position) = if total_event_lines == 0 {
                    (Vec::new(), 0)
                } else if self.event_scroll_offset == 0 {
                    let start = total_event_lines.saturating_sub(available_height);
                    let evts: Vec<Line> = all_event_lines[start..]
                        .iter()
                        .map(|line| Line::from(line.clone()))
                        .collect();
                    (evts, total_event_lines.saturating_sub(1))
                } else {
                    let end_index = total_event_lines.saturating_sub(self.event_scroll_offset);
                    let start_index = end_index.saturating_sub(available_height);
                    let evts: Vec<Line> = all_event_lines[start_index..end_index]
                        .iter()
                        .map(|line| Line::from(line.clone()))
                        .collect();
                    (evts, end_index.saturating_sub(1))
                };

                self.event_scrollbar_state = self.event_scrollbar_state
                    .content_length(total_event_lines)
                    .position(event_scroll_position);

                let event_title = if self.event_scroll_offset > 0 {
                    format!("Roon Events (↑{} lines, Alt+↑/↓ to scroll)", self.event_scroll_offset)
                } else {
                    "Roon Events (Alt+↑/↓ to scroll)".to_string()
                };
                let events_widget = Paragraph::new(visible_events)
                    .block(Block::default().borders(Borders::ALL).title(event_title))
                    .style(Style::default().fg(Color::Green))
                    .wrap(Wrap { trim: false });
                f.render_widget(events_widget, chunks[events_chunk_idx]);

                let event_scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓"));
                let event_scrollbar_area = Rect {
                    x: chunks[events_chunk_idx].x + chunks[events_chunk_idx].width - 1,
                    y: chunks[events_chunk_idx].y + 1,
                    width: 1,
                    height: chunks[events_chunk_idx].height.saturating_sub(2),
                };
                f.render_stateful_widget(event_scrollbar, event_scrollbar_area, &mut self.event_scrollbar_state);

                // Store event area for mouse click detection
                event_area = Some(chunks[events_chunk_idx]);
                }
            }

            // Determine output chunk index based on maximized state and available windows
            let (output_chunk_idx, input_chunk_idx) = match self.maximized_window {
                MaximizedWindow::Zones => (usize::MAX, 1),   // Output hidden when zones maximized
                MaximizedWindow::Events => (usize::MAX, 1),  // Output hidden when events maximized
                MaximizedWindow::Output => (0, 1),           // Output at top when maximized
                MaximizedWindow::None => {
                    match (has_zones, has_events) {
                        (true, true) => (2, 3),   // 4-pane: zones=0, events=1, output=2, input=3
                        (true, false) => (1, 2),  // 3-pane: zones=0, output=1, input=2
                        (false, true) => (1, 2),  // 3-pane: events=0, output=1, input=2
                        (false, false) => (0, 1), // 2-pane: output=0, input=1
                    }
                }
            };

            // Render messages area with scrolling support (skip if events or zones maximized)
            if self.maximized_window != MaximizedWindow::Events && self.maximized_window != MaximizedWindow::Zones {
                let messages = self.message_buffer.lock().unwrap();
                let all_messages = messages.messages();
                let total_messages = all_messages.len();

                // Calculate how many lines can fit in the output area
                let available_height = chunks[output_chunk_idx].height.saturating_sub(2) as usize;

            // Calculate which messages to show based on scroll_offset
            let (visible_messages, scroll_position) = if total_messages == 0 {
                (Vec::new(), 0)
            } else if self.scroll_offset == 0 {
                // At bottom (auto-scroll mode) - show last N messages
                let start = total_messages.saturating_sub(available_height);
                let msgs: Vec<Line> = all_messages[start..]
                    .iter()
                    .map(|msg| Line::from(msg.clone()))
                    .collect();
                (msgs, total_messages.saturating_sub(1))
            } else {
                // Scrolled up - show messages based on offset
                // scroll_offset=1 means we want to see the message at index (total-1)
                let end_index = total_messages.saturating_sub(self.scroll_offset);
                let start_index = end_index.saturating_sub(available_height);
                let msgs: Vec<Line> = all_messages[start_index..end_index]
                    .iter()
                    .map(|msg| Line::from(msg.clone()))
                    .collect();
                (msgs, end_index.saturating_sub(1))
            };

            // Update scrollbar state
            self.scrollbar_state = self.scrollbar_state
                .content_length(total_messages)
                .position(scroll_position);

            // Render messages
            let title = if self.scroll_offset > 0 {
                format!("Output (↑{} lines)", self.scroll_offset)
            } else {
                "Output".to_string()
            };
            let messages_widget = Paragraph::new(visible_messages)
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false });
            f.render_widget(messages_widget, chunks[output_chunk_idx]);

            // Render scrollbar
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let scrollbar_area = Rect {
                x: chunks[output_chunk_idx].x + chunks[output_chunk_idx].width - 1,
                y: chunks[output_chunk_idx].y + 1,
                width: 1,
                height: chunks[output_chunk_idx].height.saturating_sub(2),
            };
            f.render_stateful_widget(scrollbar, scrollbar_area, &mut self.scrollbar_state);

                // Store output area for mouse click detection
                output_area = Some(chunks[output_chunk_idx]);
            }

            // Render input area
            let prompt = self.get_prompt();
            let input_text = vec![Line::from(vec![
                Span::styled(&prompt, Style::default().fg(Color::Cyan)),
                Span::raw(&self.input),
            ])];

            let input_widget = Paragraph::new(input_text)
                .block(Block::default().borders(Borders::ALL).title("Input"));
            f.render_widget(input_widget, chunks[input_chunk_idx]);

            // Set cursor position
            f.set_cursor_position((
                chunks[input_chunk_idx].x + prompt.len() as u16 + self.cursor_position as u16 + 1,
                chunks[input_chunk_idx].y + 1,
            ));
        })?;
        Ok((zone_area, event_area, output_area))
    }
}

/// Run the terminal UI with async command handler
pub async fn run_tui_async<F, Fut, P>(
    message_buffer: Arc<StdMutex<MessageBuffer>>,
    prompt_fn: P,
    command_handler: F,
    exit_flag: Arc<StdMutex<bool>>,
    commands: Vec<String>,
    ws_rx: Option<tokio::sync::broadcast::Receiver<crate::roon::WsMessage>>,
) -> io::Result<()>
where
    F: Fn(String) -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
    P: Fn() -> String + Send + 'static,
{
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(message_buffer.clone(), prompt_fn, commands);

    // Setup event buffer and zone buffer if we have a WebSocket receiver
    if let Some(mut ws_rx) = ws_rx {
        let event_buffer = Arc::new(StdMutex::new(EventBuffer::new(1000)));
        app.set_event_buffer(event_buffer.clone());

        let zone_buffer = Arc::new(StdMutex::new(ZoneBuffer::new()));
        app.set_zone_buffer(zone_buffer.clone());

        // Spawn event listener task
        tokio::spawn(async move {
            use chrono::Local;
            while let Ok(msg) = ws_rx.recv().await {
                // Handle SeekUpdated events separately (update position but don't show in event buffer)
                if let crate::roon::WsMessage::SeekUpdated { zone_id, seek_position, .. } = &msg {
                    if let Ok(mut buffer) = zone_buffer.lock() {
                        buffer.update_position(zone_id, *seek_position);
                    }
                    continue;
                }

                // Get current log level to determine format
                let current_level = log::max_level();
                let use_debug_format = current_level == log::LevelFilter::Debug || current_level == log::LevelFilter::Trace;

                let event_str = if use_debug_format {
                    // DEBUG/TRACE: Show full event structure
                    match &msg {
                        crate::roon::WsMessage::ZonesChanged { now_playing } => {
                            format!("[{}] zones_changed:\n{:#?}", Local::now().format("%H:%M:%S"), now_playing)
                        }
                        crate::roon::WsMessage::ConnectionChanged { connected } => {
                            format!("[{}] connection_changed: {:#?}", Local::now().format("%H:%M:%S"), connected)
                        }
                        crate::roon::WsMessage::QueueChanged { zone_id } => {
                            format!("[{}] queue_changed: {:#?}", Local::now().format("%H:%M:%S"), zone_id)
                        }
                        crate::roon::WsMessage::SeekUpdated { .. } => unreachable!(),
                    }
                } else {
                    // INFO/OFF: Show simplified summary
                    match &msg {
                        crate::roon::WsMessage::ZonesChanged { now_playing } => {
                            format!("[{}] zones_changed: {} zones", Local::now().format("%H:%M:%S"), now_playing.len())
                        }
                        crate::roon::WsMessage::ConnectionChanged { connected } => {
                            format!("[{}] connection_changed: {}", Local::now().format("%H:%M:%S"), connected)
                        }
                        crate::roon::WsMessage::QueueChanged { zone_id } => {
                            format!("[{}] queue_changed: zone={}", Local::now().format("%H:%M:%S"), zone_id)
                        }
                        crate::roon::WsMessage::SeekUpdated { .. } => unreachable!(),
                    }
                };

                if let Ok(mut buffer) = event_buffer.lock() {
                    buffer.push(event_str);
                }

                // Update zone buffer when zones change
                if let crate::roon::WsMessage::ZonesChanged { now_playing } = &msg {
                    let mut zones: Vec<ZoneDisplay> = now_playing.iter().map(|z| {
                        ZoneDisplay {
                            zone_id: z.zone_id.clone(),
                            zone_name: z.zone_name.clone(),
                            state: z.state.clone(),
                            track: z.track.clone(),
                            artist: z.artist.clone(),
                            album: z.album.clone(),
                            format: z.dcs_format.clone(),
                            position_seconds: z.position_seconds,
                            length_seconds: z.length_seconds,
                        }
                    }).collect();

                    // Sort zones: playing, loading, paused, stopped, then others
                    zones.sort_by(|a, b| {
                        get_state_priority(&a.state).cmp(&get_state_priority(&b.state))
                    });

                    if let Ok(mut buffer) = zone_buffer.lock() {
                        buffer.update(zones);
                    }
                }
            }
        });
    }

    // Create channel for commands
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn command processor task using spawn_local for !Send futures
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        while let Some(command) = rx.recv().await {
            command_handler(command).await;
        }
    });

    // Main loop
    local.run_until(async {
        loop {
            // Render and get window areas for mouse click detection
            let (zone_area, event_area, output_area) = app.render(&mut terminal)?;

            // Handle input with timeout to allow other tasks to run
            let poll_result = tokio::time::timeout(
                tokio::time::Duration::from_millis(16),
                async {
                    if event::poll(std::time::Duration::from_millis(0))? {
                        match event::read()? {
                            Event::Key(key) => {
                                return Ok::<Option<String>, io::Error>(app.handle_key(key));
                            }
                            Event::Mouse(mouse) => {
                                // Note: Mouse capture is disabled to allow text selection/copy/paste
                                // Mouse events won't be received, so this code path is inactive
                                app.handle_mouse(mouse, zone_area, event_area, output_area);
                                return Ok(None);
                            }
                            _ => {}
                        }
                    }
                    Ok(None)
                }
            ).await;

            // Process command if one was entered
            if let Ok(Ok(Some(command))) = poll_result {
                let _ = tx.send(command);
            }

            // Yield to allow command processor to run
            tokio::task::yield_now().await;

            // Check exit conditions
            if app.should_exit() {
                break;
            }

            // Check if exit was requested via command
            if let Ok(should_exit) = exit_flag.lock() {
                if *should_exit {
                    break;
                }
            }
        }
        Ok::<(), io::Error>(())
    }).await?;

    // Truncate history to last 100 entries and save on exit
    let _ = truncate_and_save_history(&app.history, 100);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    Ok(())
}
