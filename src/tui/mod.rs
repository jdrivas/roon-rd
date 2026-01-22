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

/// Terminal UI application state
pub struct App {
    /// Current input string
    input: String,
    /// Cursor position in the input
    cursor_position: usize,
    /// Message buffer (shared with logger)
    message_buffer: Arc<StdMutex<MessageBuffer>>,
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
    /// Scrollbar state
    scrollbar_state: ScrollbarState,
    /// Available commands for completion
    commands: Vec<String>,
    /// Current completion candidates
    completion_candidates: Vec<String>,
    /// Current position in completion candidates
    completion_index: Option<usize>,
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
            history,
            history_position: None,
            should_exit: false,
            prompt_fn: Arc::new(StdMutex::new(Box::new(prompt_fn))),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            commands,
            completion_candidates: Vec::new(),
            completion_index: None,
        }
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

    pub fn should_exit(&self) -> bool {
        self.should_exit
    }

    /// Render the UI
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        terminal.draw(|f| {
            // Split terminal into two areas: messages (top) and input (bottom)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),      // Messages area (takes remaining space)
                    Constraint::Length(3),   // Input area (fixed 3 lines: border + input + border)
                ])
                .split(f.area());

            // Render messages area with scrolling support
            let messages = self.message_buffer.lock().unwrap();
            let all_messages = messages.messages();
            let total_messages = all_messages.len();

            // Calculate how many lines can fit in the output area
            // chunks[0].height - 2 for borders
            let available_height = chunks[0].height.saturating_sub(2) as usize;

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
            f.render_widget(messages_widget, chunks[0]);

            // Render scrollbar
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let scrollbar_area = Rect {
                x: chunks[0].x + chunks[0].width - 1,
                y: chunks[0].y + 1,
                width: 1,
                height: chunks[0].height.saturating_sub(2),
            };
            f.render_stateful_widget(scrollbar, scrollbar_area, &mut self.scrollbar_state);

            // Render input area
            let prompt = self.get_prompt();
            let input_text = vec![Line::from(vec![
                Span::styled(&prompt, Style::default().fg(Color::Cyan)),
                Span::raw(&self.input),
            ])];

            let input_widget = Paragraph::new(input_text)
                .block(Block::default().borders(Borders::ALL).title("Input"));
            f.render_widget(input_widget, chunks[1]);

            // Set cursor position
            f.set_cursor_position((
                chunks[1].x + prompt.len() as u16 + self.cursor_position as u16 + 1,
                chunks[1].y + 1,
            ));
        })?;
        Ok(())
    }
}

/// Run the terminal UI with async command handler
pub async fn run_tui_async<F, Fut, P>(
    message_buffer: Arc<StdMutex<MessageBuffer>>,
    prompt_fn: P,
    command_handler: F,
    exit_flag: Arc<StdMutex<bool>>,
    commands: Vec<String>,
) -> io::Result<()>
where
    F: Fn(String) -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
    P: Fn() -> String + Send + 'static,
{
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(message_buffer.clone(), prompt_fn, commands);

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
            // Render
            app.render(&mut terminal)?;

            // Handle input with timeout to allow other tasks to run
            let poll_result = tokio::time::timeout(
                tokio::time::Duration::from_millis(16),
                async {
                    if event::poll(std::time::Duration::from_millis(0))? {
                        if let Event::Key(key) = event::read()? {
                            return Ok::<Option<String>, io::Error>(app.handle_key(key));
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
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
