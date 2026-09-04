use std::io;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::clipboard;
use crate::config::Config;
use crate::model::{ProviderKind, SearchMode, SearchOptions, SearchResult};
use crate::search;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(450);

pub fn run(config: Config) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let terminal = ratatui::init();
    let result = App::new(config).run(terminal);
    ratatui::restore();
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    if result.is_ok() {
        println!("\x1b[2m╭────────────────────────────────────────╮\x1b[0m");
        println!(
            "\x1b[2m│\x1b[0m \x1b[36mMay the agents vibe with you.\x1b[0m       \x1b[2m│\x1b[0m"
        );
        println!("\x1b[2m╰────────────────────────────────────────╯\x1b[0m");
    }
    result
}

struct App {
    config: Config,
    query: String,
    results: Vec<SearchResult>,
    selected: usize,
    list_state: ListState,
    status: String,
    providers: Vec<ProviderKind>,
    mode: SearchMode,
    full_text: bool,
    show_preview: bool,
    show_filters: bool,
    filter_cursor: usize,
    preview_scroll: u16,
    search_due: Option<Instant>,
    searching: bool,
    search_rx: Option<Receiver<Result<Vec<SearchResult>, String>>>,
    search_seq: u64,
}

/// Rows in the filter panel, in display order.
const FILTER_ROWS: usize = ProviderKind::COUNT + 2;

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            status: String::new(),
            providers: Vec::new(),
            mode: SearchMode::Phrase,
            full_text: true,
            show_preview: false,
            show_filters: false,
            filter_cursor: 0,
            preview_scroll: 0,
            search_due: None,
            searching: false,
            search_rx: None,
            search_seq: 0,
        }
    }

    fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        // Open straight into the most recent conversations (chronological view)
        // so the tool is useful before typing anything.
        self.start_recent();
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if self.handle_key(key)? {
                        break;
                    }
                }
            }

            if self
                .search_due
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.start_search();
            }
            self.poll_search();
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.show_filters {
            return self.handle_filter_key(key);
        }
        match key.code {
            KeyCode::Esc => return Ok(true),
            KeyCode::Char('q') if key.modifiers.is_empty() && self.query.is_empty() => {
                return Ok(true);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
            KeyCode::Char(ch) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match ch {
                        's' => self.toggle_filters(),
                        'm' | 'r' => {
                            self.mode = self.mode.next();
                            self.schedule_search();
                        }
                        'f' => {
                            self.full_text = !self.full_text;
                            self.schedule_search();
                        }
                        'o' => {
                            self.open_selected()?;
                        }
                        'y' => {
                            self.copy_selected();
                        }
                        'p' => {
                            self.toggle_preview();
                        }
                        _ => {}
                    }
                } else {
                    self.query.push(ch);
                    self.schedule_search();
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                if self.query.is_empty() {
                    self.start_recent();
                } else {
                    self.schedule_search();
                }
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.scroll_preview(-6),
            KeyCode::PageDown => self.scroll_preview(6),
            KeyCode::Enter if self.query.trim().is_empty() && self.results.is_empty() => {
                self.start_recent();
            }
            KeyCode::Enter => self.copy_selected(),
            KeyCode::Tab => self.toggle_filters(),
            KeyCode::F(2) => {
                self.full_text = !self.full_text;
                self.schedule_search();
            }
            _ => {}
        }

        Ok(false)
    }

    /// Key handling while the filter panel is open.
    fn handle_filter_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => self.toggle_filters(),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toggle_filters()
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
            KeyCode::Up => {
                self.filter_cursor = (self.filter_cursor + FILTER_ROWS - 1) % FILTER_ROWS;
            }
            KeyCode::Down => {
                self.filter_cursor = (self.filter_cursor + 1) % FILTER_ROWS;
            }
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Right | KeyCode::Left => {
                self.activate_filter_row();
            }
            _ => {}
        }
        Ok(false)
    }

    fn toggle_filters(&mut self) {
        self.show_filters = !self.show_filters;
        self.filter_cursor = 0;
    }

    /// Toggle/cycle whatever filter row the cursor is on.
    fn activate_filter_row(&mut self) {
        let providers = ProviderKind::all();
        if self.filter_cursor < providers.len() {
            let provider = providers[self.filter_cursor];
            if let Some(pos) = self.providers.iter().position(|p| *p == provider) {
                self.providers.remove(pos);
            } else {
                self.providers.push(provider);
            }
        } else if self.filter_cursor == providers.len() {
            self.mode = self.mode.next();
        } else {
            self.full_text = !self.full_text;
        }
        if self.query.trim().is_empty() {
            self.start_recent();
        } else {
            self.schedule_search();
        }
    }

    fn start_search(&mut self) {
        self.search_due = None;
        if self.query.trim().len() < 2 {
            self.start_recent();
            return;
        }
        self.search_seq = self.search_seq.wrapping_add(1);
        let seq = self.search_seq;
        let config = self.config.clone();
        self.searching = true;
        self.status.clear();
        let options = SearchOptions {
            query: self.query.clone(),
            providers: self.providers.clone(),
            mode: self.mode,
            limit: 50,
            full_text: self.full_text,
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = search::search(&config, &options).map_err(|error| error.to_string());
            let _ = tx.send(if seq == 0 {
                Err("stale search".to_string())
            } else {
                result
            });
        });
        self.search_rx = Some(rx);
    }

    fn start_recent(&mut self) {
        self.search_due = None;
        self.search_seq = self.search_seq.wrapping_add(1);
        let config = self.config.clone();
        let providers = self.providers.clone();
        self.results.clear();
        self.selected = 0;
        self.list_state.select(None);
        self.preview_scroll = 0;
        self.searching = true;
        self.status.clear();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = search::recent(&config, &providers, 50).map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
        self.search_rx = Some(rx);
    }

    fn poll_search(&mut self) {
        let Some(rx) = &self.search_rx else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.search_rx = None;
        self.searching = false;
        match result {
            Ok(results) => {
                self.results = results;
                self.selected = self.selected.min(self.results.len().saturating_sub(1));
                self.list_state
                    .select((!self.results.is_empty()).then_some(self.selected));
            }
            Err(error) => {
                self.status = error.to_string();
            }
        }
    }

    fn schedule_search(&mut self) {
        if self.query.trim().len() < 2 {
            self.search_due = None;
            self.results.clear();
            self.selected = 0;
            self.list_state.select(None);
            self.status.clear();
            return;
        }
        self.search_due = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.status.clear();
    }

    fn toggle_preview(&mut self) {
        self.show_preview = !self.show_preview;
        self.preview_scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(len)) as usize;
        self.list_state.select(Some(self.selected));
        self.preview_scroll = 0;
    }

    fn scroll_preview(&mut self, delta: i16) {
        if !self.show_preview {
            return;
        }
        if delta.is_negative() {
            self.preview_scroll = self.preview_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.preview_scroll = self.preview_scroll.saturating_add(delta as u16);
        }
    }

    fn selected(&self) -> Option<&SearchResult> {
        self.results.get(self.selected)
    }

    fn copy_selected(&mut self) {
        let Some(result) = self.selected() else {
            return;
        };
        let command = result.resume_command.clone();
        match clipboard::copy(&command) {
            Ok(()) => self.status = format!("Copied: {command}"),
            Err(error) => self.status = format!("Clipboard error: {error}"),
        }
    }

    fn open_selected(&mut self) -> Result<()> {
        let Some(result) = self.selected() else {
            return Ok(());
        };
        let command = result.resume_command.clone();
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;
        Command::new("sh").arg("-lc").arg(&command).status()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        self.status = format!("Opened: {command}");
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Min(8),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_search(frame, chunks[0]);
        self.draw_hint(frame, chunks[1]);
        self.draw_body(frame, chunks[2]);
        self.draw_status(frame, chunks[3]);

        if self.show_filters {
            self.draw_filters(frame, area);
        }
    }

    fn provider_summary(&self) -> String {
        if self.providers.is_empty() {
            "all".to_string()
        } else {
            self.providers
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>()
                .join(",")
        }
    }

    fn draw_search(&self, frame: &mut Frame, area: Rect) {
        let title = format!(
            " Fainder  providers:{}  mode:{}  scope:{} ",
            self.provider_summary(),
            self.mode.label(),
            if self.full_text {
                "all text"
            } else {
                "titles only"
            }
        );
        let input = Paragraph::new(self.query.as_str())
            .style(Style::default().fg(Color::White))
            .block(
                rounded_block()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(Color::Cyan)),
            );
        frame.render_widget(input, area);
    }

    /// One-line description of exactly what the current query/mode will match.
    fn draw_hint(&self, frame: &mut Frame, area: Rect) {
        let line = if self.query.trim().is_empty() {
            Line::from(vec![
                Span::styled(
                    "Showing recent conversations  ",
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    "· type to search · Ctrl-s filters · Ctrl-m mode",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("▸ ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    self.mode.describe(&self.query),
                    Style::default().fg(Color::Cyan),
                ),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    /// Centered overlay panel for choosing providers, mode, and scope.
    fn draw_filters(&self, frame: &mut Frame, area: Rect) {
        let providers = ProviderKind::all();
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "Providers  (Space to toggle, empty = all)",
            Style::default().fg(Color::DarkGray),
        )));
        for (index, provider) in providers.iter().enumerate() {
            let checked = self.providers.is_empty() || self.providers.contains(provider);
            lines.push(filter_row(
                index == self.filter_cursor,
                &format!("[{}] {}", if checked { "x" } else { " " }, provider.label()),
                provider_color(*provider),
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Mode",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(filter_row(
            self.filter_cursor == providers.len(),
            &format!("‹ {} ›   (phrase · words · regex)", self.mode.label()),
            Color::White,
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Scope",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(filter_row(
            self.filter_cursor == providers.len() + 1,
            if self.full_text {
                "‹ all text ›   (titles+paths+content)"
            } else {
                "‹ titles only ›   (titles+paths+recent)"
            },
            Color::White,
        ));

        let rect = centered_rect(54, 18, area);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                rounded_block()
                    .borders(Borders::ALL)
                    .title(" Filters  ↑↓ move · Space toggle · Esc close ")
                    .border_style(Style::default().fg(Color::Magenta)),
            ),
            rect,
        );
    }

    fn draw_body(&mut self, frame: &mut Frame, area: Rect) {
        if self.show_preview {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(area);
            self.draw_results(frame, chunks[0]);
            self.draw_preview(frame, chunks[1]);
        } else {
            self.draw_results(frame, area);
        }
    }

    fn draw_results(&mut self, frame: &mut Frame, area: Rect) {
        if self.query.trim().is_empty() && self.results.is_empty() && !self.searching {
            self.draw_empty_help(frame, area);
            return;
        }

        if (self.search_due.is_some() || self.searching) && self.results.is_empty() {
            self.draw_searching(frame, area);
            return;
        }

        let items = self
            .results
            .iter()
            .map(|result| {
                let started = format_started_time(result.created_at);
                let updated = format_last_used_time(result.updated_at);
                let cwd = result
                    .cwd
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|p| p.to_str())
                    .unwrap_or("-");
                let messages = result
                    .message_count
                    .map(|count| format!("{count} msgs"))
                    .unwrap_or_else(|| "-- msgs".to_string());
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            format!("{:<8}", result.provider.label()),
                            Style::default()
                                .fg(provider_color(result.provider))
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{:<18}", truncate(cwd, 18)),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::raw("  "),
                        Span::raw(result.title.clone()),
                    ]),
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(messages, Style::default().fg(Color::DarkGray)),
                        Span::styled("  started ", Style::default().fg(Color::DarkGray)),
                        Span::styled(started, Style::default().fg(Color::Gray)),
                        Span::styled("  last ", Style::default().fg(Color::DarkGray)),
                        Span::styled(updated, Style::default().fg(Color::Gray)),
                    ]),
                ])
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                conversations_block(self.conversations_title())
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_empty_help(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(Span::styled(
                "Find a conversation to resume",
                Style::default().fg(Color::Rgb(142, 202, 230)),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(Color::Gray)),
                Span::styled("Enter", Style::default().fg(Color::LightYellow)),
                Span::styled(
                    " to show your most recent conversations.",
                    Style::default().fg(Color::Gray),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Search for ", Style::default().fg(Color::Gray)),
                Span::styled("a project", Style::default().fg(Color::LightYellow)),
                Span::styled(", ", Style::default().fg(Color::Gray)),
                Span::styled("repo", Style::default().fg(Color::LightGreen)),
                Span::styled(", ", Style::default().fg(Color::Gray)),
                Span::styled("bug", Style::default().fg(Color::LightMagenta)),
                Span::styled(", client, or topic.", Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                "Fainder reads local Codex, Claude Code, OpenCode, Hermes, Cursor, and Copilot histories.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Examples  ", Style::default().fg(Color::DarkGray)),
                Span::styled("SmartUp", Style::default().fg(Color::Rgb(142, 202, 230))),
                Span::styled("   bedrock latency   ", Style::default().fg(Color::Gray)),
                Span::styled("shapeup tasks", Style::default().fg(Color::LightGreen)),
            ]),
            Line::from(vec![
                Span::styled(
                    "By default the whole phrase is matched. ",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled("Ctrl-m", Style::default().fg(Color::Gray)),
                Span::styled(
                    " switches phrase → words → regex.",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Controls  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl-s/Tab", Style::default().fg(Color::Gray)),
                Span::styled(" filters   ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl-m", Style::default().fg(Color::Gray)),
                Span::styled(" mode   ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl-f", Style::default().fg(Color::Gray)),
                Span::styled(" scope   ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl-p", Style::default().fg(Color::Gray)),
                Span::styled(" preview", Style::default().fg(Color::DarkGray)),
            ]),
        ];
        let help = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            conversations_block(self.conversations_title())
                .border_style(Style::default().fg(Color::Blue)),
        );
        frame.render_widget(help, area);
    }

    fn draw_searching(&self, frame: &mut Frame, area: Rect) {
        let title = self.conversations_title();
        let message = if self.search_due.is_some() {
            "Waiting for you to stop typing..."
        } else {
            "Searching conversations..."
        };
        let help = Paragraph::new(vec![
            Line::from(Span::styled(
                message,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Results will appear here as soon as the search finishes."),
        ])
        .wrap(Wrap { trim: true })
        .block(conversations_block(title).border_style(Style::default().fg(Color::Blue)));
        frame.render_widget(help, area);
    }

    fn draw_preview(&self, frame: &mut Frame, area: Rect) {
        let lines = if let Some(result) = self.selected() {
            let cwd = result
                .cwd
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".to_string());
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        result.provider.label(),
                        Style::default()
                            .fg(provider_color(result.provider))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("  {}", result.title)),
                ]),
                Line::from(Span::styled(cwd, Style::default().fg(Color::Yellow))),
                Line::from(Span::styled(
                    result.resume_command.clone(),
                    Style::default().fg(Color::Green),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Snippets",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
            ];
            for snippet in &result.snippets {
                lines.push(Line::from(snippet.clone()));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Recent User Messages",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            let latest_start = result.latest_messages.len().saturating_sub(3);
            for message in &result.latest_messages[latest_start..] {
                lines.push(Line::from(message.clone()));
            }
            lines
        } else {
            vec![Line::from(
                "Type at least two characters to search, or press Enter for recent conversations.",
            )]
        };
        let preview = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .scroll((self.preview_scroll, 0))
            .block(
                rounded_block()
                    .borders(Borders::ALL)
                    .title(" Preview  PgUp/PgDn ")
                    .border_style(Style::default().fg(Color::Magenta)),
            );
        frame.render_widget(Clear, area);
        frame.render_widget(preview, area);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let help = "Enter copy   Ctrl-o open   Ctrl-p preview   Ctrl-s/Tab filters   Ctrl-m mode   Ctrl-f scope   Esc quit";
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(help, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if self.status.is_empty() { "" } else { "   " },
                    Style::default(),
                ),
                Span::styled(self.status.clone(), Style::default().fg(Color::Yellow)),
            ])),
            area,
        );
    }

    fn conversations_title(&self) -> String {
        if self.search_due.is_some() {
            " Conversations  ⌕ waiting ".to_string()
        } else if self.searching {
            " Conversations  ⌕ searching ".to_string()
        } else if self.query.trim().is_empty() && !self.results.is_empty() {
            format!(" Recent Conversations  {} results ", self.results.len())
        } else if !self.results.is_empty() {
            format!(" Conversations  {} results ", self.results.len())
        } else {
            " Conversations ".to_string()
        }
    }
}

fn provider_color(provider: ProviderKind) -> Color {
    match provider {
        ProviderKind::Codex => Color::Cyan,
        ProviderKind::Claude => Color::Rgb(255, 136, 0),
        ProviderKind::Opencode => Color::LightGreen,
        ProviderKind::Hermes => Color::LightMagenta,
        ProviderKind::Cursor => Color::LightYellow,
        ProviderKind::Copilot => Color::LightBlue,
        ProviderKind::Kiro => Color::Green,
    }
}

fn rounded_block<'a>() -> Block<'a> {
    Block::default().border_set(symbols::border::ROUNDED)
}

/// A single selectable row inside the filter panel.
fn filter_row<'a>(selected: bool, text: &str, color: Color) -> Line<'a> {
    let marker = if selected { "› " } else { "  " };
    let style = if selected {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(Color::Magenta)),
        Span::styled(text.to_string(), style),
    ])
}

/// A fixed-size rectangle centered inside `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn conversations_block<'a>(title: String) -> Block<'a> {
    rounded_block().borders(Borders::ALL).title(title)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    format!(
        "{}…",
        value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>()
    )
}

fn format_started_time(value: Option<DateTime<Utc>>) -> String {
    let Some(value) = value else {
        return "--".to_string();
    };
    let now = Utc::now();
    let age = now.signed_duration_since(value);
    if age.num_hours() < 24 {
        value.format("%H:%M").to_string()
    } else if age.num_days() == 1 {
        "yesterday".to_string()
    } else if age.num_days() < 7 {
        format!("{}d ago", age.num_days())
    } else if value.year() == now.year() {
        value.format("%b %-d").to_string()
    } else {
        value.format("%Y-%m-%d").to_string()
    }
}

fn format_last_used_time(value: Option<DateTime<Utc>>) -> String {
    let Some(value) = value else {
        return "--".to_string();
    };
    let now = Utc::now();
    let age = now.signed_duration_since(value);
    if age.num_minutes() < 1 {
        "now".to_string()
    } else if age.num_hours() < 1 {
        format!("{}m ago", age.num_minutes())
    } else if age.num_hours() < 24 {
        format!("{}h ago", age.num_hours())
    } else if age.num_days() == 1 {
        "yesterday".to_string()
    } else if age.num_days() < 7 {
        format!("{}d ago", age.num_days())
    } else if value.year() == now.year() {
        value.format("%b %-d").to_string()
    } else {
        value.format("%Y-%m-%d").to_string()
    }
}
