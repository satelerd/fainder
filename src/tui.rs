use std::io;
use std::process::Command;
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
use crate::model::{ProviderKind, SearchOptions, SearchResult};
use crate::search;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(800);

pub fn run(config: Config) -> Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let terminal = ratatui::init();
    let result = App::new(config).run(terminal);
    ratatui::restore();
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
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
    regex: bool,
    full_text: bool,
    show_preview: bool,
    preview_scroll: u16,
    search_due: Option<Instant>,
    searching: bool,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            status: "Type a query to search local agent conversations.".to_string(),
            providers: Vec::new(),
            regex: false,
            full_text: true,
            show_preview: false,
            preview_scroll: 0,
            search_due: None,
            searching: false,
        }
    }

    fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
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
                self.search_now();
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => return Ok(true),
            KeyCode::Char('q') if key.modifiers.is_empty() && self.query.is_empty() => {
                return Ok(true);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),
            KeyCode::Char(ch) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match ch {
                        'r' => {
                            self.regex = !self.regex;
                            self.search_now();
                        }
                        'f' => {
                            self.full_text = !self.full_text;
                            self.search_now();
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
                    self.results.clear();
                    self.selected = 0;
                    self.search_due = None;
                    self.status = "Type a query to search local agent conversations.".to_string();
                } else {
                    self.schedule_search();
                }
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.scroll_preview(-6),
            KeyCode::PageDown => self.scroll_preview(6),
            KeyCode::Enter => self.copy_selected(),
            KeyCode::Tab => self.cycle_provider(),
            KeyCode::F(2) => {
                self.full_text = !self.full_text;
                self.search_now();
            }
            _ => {}
        }

        Ok(false)
    }

    fn search_now(&mut self) {
        self.search_due = None;
        if self.query.trim().len() < 2 {
            self.results.clear();
            self.selected = 0;
            self.list_state.select(None);
            self.status = "Type at least two characters.".to_string();
            return;
        }
        self.searching = true;
        let options = SearchOptions {
            query: self.query.clone(),
            providers: self.providers.clone(),
            regex: self.regex,
            limit: 50,
            full_text: self.full_text,
        };
        match search::search(&self.config, &options) {
            Ok(results) => {
                self.results = results;
                self.selected = self.selected.min(self.results.len().saturating_sub(1));
                self.list_state
                    .select((!self.results.is_empty()).then_some(self.selected));
                self.status = format!("{} results", self.results.len());
            }
            Err(error) => {
                self.status = error.to_string();
            }
        }
        self.searching = false;
    }

    fn schedule_search(&mut self) {
        if self.query.trim().len() < 2 {
            self.search_due = None;
            self.results.clear();
            self.selected = 0;
            self.list_state.select(None);
            self.status = "Type at least two characters.".to_string();
            return;
        }
        self.search_due = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.status = "Waiting for typing to pause...".to_string();
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

    fn cycle_provider(&mut self) {
        self.providers = match self.providers.as_slice() {
            [] => vec![ProviderKind::Codex],
            [ProviderKind::Codex] => vec![ProviderKind::Claude],
            [ProviderKind::Claude] => vec![ProviderKind::Opencode],
            [ProviderKind::Opencode] => vec![ProviderKind::Hermes],
            [ProviderKind::Hermes] => Vec::new(),
            _ => Vec::new(),
        };
        self.schedule_search();
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_search(frame, chunks[0]);
        self.draw_body(frame, chunks[1]);
        self.draw_status(frame, chunks[2]);
    }

    fn draw_search(&self, frame: &mut Frame, area: Rect) {
        let provider = if self.providers.is_empty() {
            "all".to_string()
        } else {
            self.providers
                .iter()
                .map(|p| p.label())
                .collect::<Vec<_>>()
                .join(",")
        };
        let marker = if self.searching {
            "⌕ searching  "
        } else if self.search_due.is_some() {
            "⌕ queued  "
        } else {
            ""
        };
        let title = format!(
            " Fainder  {marker}provider:{provider}  mode:{}  scope:{} ",
            if self.regex { "regex" } else { "words" },
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
        if self.query.trim().is_empty() {
            self.draw_empty_help(frame, area);
            return;
        }

        if self.search_due.is_some() && self.results.is_empty() {
            self.draw_searching(frame, area);
            return;
        }

        let items = self
            .results
            .iter()
            .map(|result| {
                let updated = format_relative_time(result.updated_at);
                let cwd = result
                    .cwd
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|p| p.to_str())
                    .unwrap_or("-");
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<8}", result.provider.label()),
                        Style::default()
                            .fg(provider_color(result.provider))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{updated}  "), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:<18}", truncate(cwd, 18)),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw("  "),
                    Span::raw(result.title.clone()),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(
                rounded_block()
                    .borders(Borders::ALL)
                    .title(" Conversations ")
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_empty_help(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(Span::styled(
                "Search local agent conversations",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Type a word like imalab, bedrock, shapeup, or a repo/client name."),
            Line::from("Multiple words are AND-matched: imalab latency finds both words."),
            Line::from("Ctrl-r switches to regex mode for patterns like imalab|bedrock."),
            Line::from("Ctrl-f toggles scope: titles only vs all transcript text."),
            Line::from("Tab cycles providers: all, codex, claude, opencode, hermes."),
            Line::from("Enter copies the resume command. Ctrl-o opens it directly."),
            Line::from("Ctrl-p opens preview; PgUp/PgDn scrolls preview text."),
            Line::from(""),
            Line::from(Span::styled(
                "No database. No indexing. Fainder reads provider histories live.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let help = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            rounded_block()
                .borders(Borders::ALL)
                .title(" Conversations ")
                .border_style(Style::default().fg(Color::Blue)),
        );
        frame.render_widget(help, area);
    }

    fn draw_searching(&self, frame: &mut Frame, area: Rect) {
        let help = Paragraph::new(vec![
            Line::from(Span::styled(
                "⌕ waiting for typing to pause...",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(
                "Fainder waits briefly before searching so the machine does not lag while you type.",
            ),
        ])
        .wrap(Wrap { trim: true })
        .block(
            rounded_block()
                .borders(Borders::ALL)
                .title(" Conversations ")
                .border_style(Style::default().fg(Color::Blue)),
        );
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
                "Latest",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for message in &result.latest_messages {
                lines.push(Line::from(message.clone()));
            }
            lines
        } else {
            vec![Line::from("Type at least two characters to search.")]
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
        let help = format!(
            "{}  |  Enter copy  Ctrl-p preview  Ctrl-o open  Ctrl-y copy  Tab provider  Ctrl-r regex  Ctrl-f scope  Esc quit",
            self.status
        );
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
            area,
        );
    }
}

fn provider_color(provider: ProviderKind) -> Color {
    match provider {
        ProviderKind::Codex => Color::Cyan,
        ProviderKind::Claude => Color::Rgb(255, 136, 0),
        ProviderKind::Opencode => Color::LightGreen,
        ProviderKind::Hermes => Color::LightMagenta,
    }
}

fn rounded_block<'a>() -> Block<'a> {
    Block::default().border_set(symbols::border::ROUNDED)
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

fn format_relative_time(value: Option<DateTime<Utc>>) -> String {
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
