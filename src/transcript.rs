use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, TimeZone, Utc};
use regex::{Regex, RegexBuilder};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use shell_escape::escape;
use walkdir::WalkDir;

use crate::config::Config;
use crate::model::{ProviderKind, Session, Transcript, TranscriptRole, TranscriptTurn};
use crate::providers;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoleFilter {
    User,
    Agent,
    Tool,
    System,
    All,
}

impl RoleFilter {
    pub fn matches(self, role: TranscriptRole) -> bool {
        match self {
            Self::User => role == TranscriptRole::User,
            Self::Agent => role == TranscriptRole::Agent,
            Self::Tool => role == TranscriptRole::Tool,
            Self::System => role == TranscriptRole::System,
            Self::All => true,
        }
    }
}

impl fmt::Display for RoleFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(role_filter_label(*self))
    }
}

impl FromStr for RoleFilter {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "user" | "human" => Ok(Self::User),
            "agent" | "assistant" => Ok(Self::Agent),
            "tool" | "tools" => Ok(Self::Tool),
            "system" => Ok(Self::System),
            "all" => Ok(Self::All),
            _ => Err(anyhow!("unknown role: {value}")),
        }
    }
}

pub struct InspectOptions {
    pub session_ref: String,
    pub role: RoleFilter,
    pub find: Option<String>,
    pub regex: bool,
    pub timeline: bool,
    pub turn: Option<usize>,
    pub around: Option<usize>,
    pub context: usize,
    pub tail: Option<usize>,
    pub limit: usize,
    pub json: bool,
    pub expand: bool,
}

pub struct ContextOptions {
    pub session_ref: String,
    pub from_turn: Option<usize>,
    pub to_turn: Option<usize>,
    pub around: Option<usize>,
    pub context: usize,
    pub tail: Option<usize>,
    pub role: RoleFilter,
    pub confirm: bool,
    pub max_tokens: usize,
    pub max_chars: Option<usize>,
    pub no_tools: bool,
    pub truncate_tools: bool,
    pub json: bool,
}

pub fn inspect(config: &Config, options: InspectOptions) -> Result<()> {
    let transcript = load_transcript_ref(config, &options.session_ref)?;
    let mut turns = selected_inspect_turns(&transcript.turns, &options)?;

    if let Some(pattern) = &options.find {
        let matcher = TextMatcher::new(pattern, options.regex)?;
        turns.retain(|turn| matcher.is_match(&turn.search_text()));
    }

    if let Some(tail) = options.tail {
        turns = take_tail(turns, tail);
    } else if options.limit > 0 && turns.len() > options.limit {
        turns.truncate(options.limit);
    }

    if options.json {
        // Bound each turn unless --expand is set. Full untruncated JSON of a
        // --tail/--find selection can balloon to 100k+ tokens and get cut off
        // by the calling harness; previews keep the payload agent-friendly.
        let turns = turns
            .iter()
            .map(|turn| bound_turn(turn, options.expand, INSPECT_JSON_PREVIEW_CHARS))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&InspectOutput {
                provider: transcript.session.provider,
                id: transcript.session.id.clone(),
                title: transcript.session.title.clone(),
                cwd: transcript
                    .session
                    .cwd
                    .as_ref()
                    .map(|path| path.display().to_string()),
                total_turns: transcript.turns.len(),
                truncated: !options.expand,
                turns,
            })?
        );
        return Ok(());
    }

    print_inspect_header(&transcript, &turns, &options);
    for turn in turns {
        print_turn_preview(&turn, options.expand);
    }
    Ok(())
}

pub fn context(config: &Config, options: ContextOptions) -> Result<()> {
    let transcript = load_transcript_ref(config, &options.session_ref)?;
    let turns = selected_context_turns(&transcript.turns, &options);
    let rendered = if options.json {
        serde_json::to_string_pretty(&ContextOutput {
            provider: transcript.session.provider,
            id: transcript.session.id.clone(),
            title: transcript.session.title.clone(),
            cwd: transcript
                .session
                .cwd
                .as_ref()
                .map(|path| path.display().to_string()),
            source_path: transcript
                .session
                .transcript_path
                .as_ref()
                .or(transcript.session.source_path.as_ref())
                .map(|path| path.display().to_string()),
            resume_command: transcript.session.resume_command.clone(),
            total_turns: transcript.turns.len(),
            estimated_tokens: estimated_tokens(&turns),
            turns: clean_context_turns(&turns, &options),
        })?
    } else {
        render_context_markdown(&transcript, &turns, &options)
    };

    let estimated = estimate_tokens(rendered.len());
    let max_tokens = options.max_tokens.max(1);
    if !options.confirm && estimated > max_tokens {
        print_context_preflight(&transcript, &options, turns.len(), estimated, max_tokens);
        return Ok(());
    }

    if let Some(max_chars) = options.max_chars {
        if rendered.len() > max_chars {
            println!(
                "{}\n\n[truncated at {max_chars} chars]",
                &rendered[..safe_boundary(&rendered, max_chars)]
            );
            return Ok(());
        }
    }
    println!("{rendered}");
    Ok(())
}

fn selected_inspect_turns<'a>(
    turns: &'a [TranscriptTurn],
    options: &InspectOptions,
) -> Result<Vec<&'a TranscriptTurn>> {
    let mut selected = if let Some(turn) = options.turn.or(options.around) {
        window(turns, turn, options.context)
    } else {
        turns.iter().collect()
    };

    if !options.timeline {
        selected.retain(|turn| options.role.matches(turn.role));
    }
    Ok(selected)
}

fn selected_context_turns<'a>(
    turns: &'a [TranscriptTurn],
    options: &ContextOptions,
) -> Vec<&'a TranscriptTurn> {
    let mut selected = if let Some(around) = options.around {
        window(turns, around, options.context)
    } else if let Some(tail) = options.tail {
        take_tail_refs(turns, tail)
    } else {
        let from = options.from_turn.unwrap_or(1);
        let to = options.to_turn.unwrap_or(turns.len());
        turns
            .iter()
            .filter(|turn| turn.turn >= from && turn.turn <= to)
            .collect()
    };

    selected.retain(|turn| {
        options.role.matches(turn.role) && (!options.no_tools || turn.role != TranscriptRole::Tool)
    });
    selected
}

fn load_transcript_ref(config: &Config, session_ref: &str) -> Result<Transcript> {
    let (provider, id) = parse_session_ref(session_ref)?;
    load_transcript(config, provider, &id)
}

fn load_transcript(config: &Config, provider: ProviderKind, id: &str) -> Result<Transcript> {
    let session = find_session(config, provider, id)?;
    let turns = match provider {
        ProviderKind::Codex => load_file_transcript(&session, codex_turn_from_value)?,
        ProviderKind::Claude => load_file_transcript(&session, claude_turns_from_value)?,
        ProviderKind::Hermes => load_file_transcript(&session, hermes_turns_from_value)?,
        ProviderKind::Opencode => load_opencode_transcript(&session)?,
        ProviderKind::Cursor | ProviderKind::Copilot => load_vscode_transcript(&session)?,
    };
    Ok(Transcript { session, turns })
}

fn find_session(config: &Config, provider: ProviderKind, id: &str) -> Result<Session> {
    let sessions = providers::sessions(config, provider).unwrap_or_default();
    if let Some(session) = sessions
        .iter()
        .find(|session| session.id == id || session.id.starts_with(id))
        .cloned()
    {
        if session.transcript_path.is_some() || provider == ProviderKind::Opencode {
            return Ok(session);
        }
    }

    let root = config.path(provider);
    let path = match provider {
        ProviderKind::Codex => find_file_by_id(&root, id, &["jsonl"])?,
        ProviderKind::Claude => find_file_by_id(&root.join("projects"), id, &["jsonl"])?,
        ProviderKind::Hermes => find_file_by_id(&root, id, &["jsonl", "json"])?,
        ProviderKind::Opencode | ProviderKind::Cursor | ProviderKind::Copilot => None,
    };

    if let Some(path) = path {
        let mut session = sessions
            .into_iter()
            .find(|session| session.id == id || session.id.starts_with(id))
            .unwrap_or_else(|| fallback_session(provider, id, &path));
        session.transcript_path = Some(path.clone());
        session.source_path.get_or_insert(path);
        return Ok(session);
    }

    sessions
        .into_iter()
        .find(|session| session.id == id || session.id.starts_with(id))
        .ok_or_else(|| anyhow!("session not found: {}:{}", provider, id))
}

fn fallback_session(provider: ProviderKind, id: &str, path: &Path) -> Session {
    Session {
        provider,
        id: id.to_string(),
        title: id.to_string(),
        cwd: None,
        created_at: None,
        updated_at: fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(DateTime::<Utc>::from),
        message_count: None,
        source_path: Some(path.to_path_buf()),
        transcript_path: Some(path.to_path_buf()),
        resume_command: match provider {
            ProviderKind::Codex => format!("codex resume {}", shell(id)),
            ProviderKind::Claude => format!("claude --resume {}", shell(id)),
            ProviderKind::Opencode => format!("opencode --session {}", shell(id)),
            ProviderKind::Hermes => format!("hermes --resume {}", shell(id)),
            ProviderKind::Cursor => "cursor".to_string(),
            ProviderKind::Copilot => "code".to_string(),
        },
        latest_messages: Vec::new(),
    }
}

fn load_file_transcript<F>(session: &Session, parser: F) -> Result<Vec<TranscriptTurn>>
where
    F: Fn(&Value) -> Vec<PartialTurn>,
{
    let path = session
        .transcript_path
        .as_ref()
        .or(session.source_path.as_ref())
        .ok_or_else(|| anyhow!("session has no transcript path"))?;
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut turns = Vec::new();
    for line in reader.lines().map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        for partial in parser(&value) {
            if partial.text.trim().is_empty()
                && partial
                    .tool_input
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
                && partial
                    .tool_result
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                continue;
            }
            turns.push(TranscriptTurn {
                turn: turns.len() + 1,
                role: partial.role,
                timestamp: partial.timestamp,
                text: one_line(&partial.text),
                tool_name: partial.tool_name,
                tool_input: partial.tool_input.map(|text| one_line(&text)),
                tool_result: partial.tool_result.map(|text| one_line(&text)),
            });
        }
    }
    Ok(turns)
}

fn load_opencode_transcript(session: &Session) -> Result<Vec<TranscriptTurn>> {
    let path = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("OpenCode session has no database path"))?;
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare(
        "select data, time_created from part where session_id = ? order by time_created",
    )?;
    let rows = stmt.query_map([session.id.as_str()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut turns = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        let (raw, created) = row;
        let value = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
        let text = text_from_value(&value);
        if text.trim().is_empty() {
            continue;
        }
        turns.push(TranscriptTurn {
            turn: turns.len() + 1,
            role: infer_role_from_value(&value),
            timestamp: millis(created),
            text: one_line(&text),
            tool_name: string_field(&value, "tool").or_else(|| string_field(&value, "type")),
            tool_input: None,
            tool_result: None,
        });
    }
    Ok(turns)
}

fn load_vscode_transcript(session: &Session) -> Result<Vec<TranscriptTurn>> {
    let path = session
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow!("VS Code-style session has no database path"))?;
    let conn = Connection::open(path)?;
    let mut turns = Vec::new();
    for key in ["aiService.prompts", "aiService.generations"] {
        let Ok(raw) = conn.query_row(
            "select cast(value as text) from ItemTable where key = ?",
            [key],
            |row| row.get::<_, String>(0),
        ) else {
            continue;
        };
        let values = serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default();
        for value in values {
            let text = text_from_value(&value);
            if text.trim().is_empty() {
                continue;
            }
            turns.push(TranscriptTurn {
                turn: turns.len() + 1,
                role: if key.contains("prompt") {
                    TranscriptRole::User
                } else {
                    TranscriptRole::Agent
                },
                timestamp: int_field(&value, "timestamp")
                    .or_else(|| int_field(&value, "createdAt"))
                    .and_then(millis),
                text: one_line(&text),
                tool_name: None,
                tool_input: None,
                tool_result: None,
            });
        }
    }
    turns.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.turn.cmp(&b.turn))
    });
    for (index, turn) in turns.iter_mut().enumerate() {
        turn.turn = index + 1;
    }
    Ok(turns)
}

#[derive(Clone)]
struct PartialTurn {
    role: TranscriptRole,
    timestamp: Option<DateTime<Utc>>,
    text: String,
    tool_name: Option<String>,
    tool_input: Option<String>,
    tool_result: Option<String>,
}

fn codex_turn_from_value(value: &Value) -> Vec<PartialTurn> {
    let timestamp = timestamp(value);
    match value.get("type").and_then(Value::as_str) {
        Some("event_msg") => {
            let Some(payload) = value.get("payload") else {
                return Vec::new();
            };
            let role = match payload.get("type").and_then(Value::as_str) {
                Some("user_message") => TranscriptRole::User,
                Some("agent_message") => TranscriptRole::Agent,
                Some("task_started") | Some("token_count") => return Vec::new(),
                _ => return Vec::new(),
            };
            vec![PartialTurn {
                role,
                timestamp,
                text: string_field(payload, "message").unwrap_or_else(|| text_from_value(payload)),
                tool_name: None,
                tool_input: None,
                tool_result: None,
            }]
        }
        Some("response_item") => {
            let Some(payload) = value.get("payload") else {
                return Vec::new();
            };
            let item_type = payload.get("type").and_then(Value::as_str);
            let role = match item_type {
                Some("function_call") | Some("function_call_output") => TranscriptRole::Tool,
                Some("message") => return Vec::new(),
                Some("reasoning") => return Vec::new(),
                _ => return Vec::new(),
            };
            let tool_name = string_field(payload, "name").or_else(|| string_field(payload, "type"));
            let tool_input = payload
                .get("arguments")
                .or_else(|| payload.get("call_id"))
                .map(text_from_value);
            let tool_result = payload.get("output").map(text_from_value);
            let text = if tool_input.is_some() || tool_result.is_some() {
                String::new()
            } else {
                text_from_value(payload.get("content").unwrap_or(payload))
            };
            vec![PartialTurn {
                role,
                timestamp,
                text,
                tool_name,
                tool_input,
                tool_result,
            }]
        }
        Some("session_meta") => Vec::new(),
        _ => Vec::new(),
    }
}

fn claude_turns_from_value(value: &Value) -> Vec<PartialTurn> {
    let timestamp = timestamp(value);
    let message_type = value.get("type").and_then(Value::as_str);
    match message_type {
        Some("user") => {
            let message = value.get("message").unwrap_or(value);
            let content = message.get("content").unwrap_or(message);
            if contains_tool_result(content) {
                vec![PartialTurn {
                    role: TranscriptRole::Tool,
                    timestamp,
                    text: String::new(),
                    tool_name: Some("tool_result".to_string()),
                    tool_input: None,
                    tool_result: Some(text_from_value(content)),
                }]
            } else if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
                Vec::new()
            } else {
                let text = text_from_value(content);
                if is_local_command_noise(&text) {
                    return Vec::new();
                }
                vec![PartialTurn {
                    role: TranscriptRole::User,
                    timestamp,
                    text,
                    tool_name: None,
                    tool_input: None,
                    tool_result: None,
                }]
            }
        }
        Some("assistant") => {
            let content = value
                .get("message")
                .and_then(|m| m.get("content"))
                .unwrap_or(value);
            let mut turns = Vec::new();
            if let Some(items) = content.as_array() {
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("text") => turns.push(PartialTurn {
                            role: TranscriptRole::Agent,
                            timestamp,
                            text: text_from_value(item),
                            tool_name: None,
                            tool_input: None,
                            tool_result: None,
                        }),
                        Some("tool_use") => turns.push(PartialTurn {
                            role: TranscriptRole::Tool,
                            timestamp,
                            text: text_from_value(item.get("input").unwrap_or(item)),
                            tool_name: string_field(item, "name"),
                            tool_input: item.get("input").map(text_from_value),
                            tool_result: None,
                        }),
                        Some("thinking") => {}
                        _ => {}
                    }
                }
            } else {
                turns.push(PartialTurn {
                    role: TranscriptRole::Agent,
                    timestamp,
                    text: text_from_value(content),
                    tool_name: None,
                    tool_input: None,
                    tool_result: None,
                });
            }
            turns
        }
        Some("system") => Vec::new(),
        _ => Vec::new(),
    }
}

fn hermes_turns_from_value(value: &Value) -> Vec<PartialTurn> {
    let timestamp = timestamp(value);
    let role = match value
        .get("role")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
    {
        Some("user") | Some("human") => TranscriptRole::User,
        Some("assistant") | Some("agent") => TranscriptRole::Agent,
        Some("tool") => TranscriptRole::Tool,
        Some("system") => TranscriptRole::System,
        _ => TranscriptRole::Unknown,
    };
    vec![PartialTurn {
        role,
        timestamp,
        text: text_from_value(
            value
                .get("content")
                .or_else(|| value.get("message"))
                .unwrap_or(value),
        ),
        tool_name: string_field(value, "tool").or_else(|| string_field(value, "name")),
        tool_input: value.get("input").map(text_from_value),
        tool_result: value
            .get("output")
            .or_else(|| value.get("result"))
            .map(text_from_value),
    }]
}

fn parse_session_ref(value: &str) -> Result<(ProviderKind, String)> {
    let Some((provider, id)) = value.split_once(':') else {
        bail!("session must be formatted as <provider>:<id>");
    };
    Ok((provider.parse()?, id.to_string()))
}

fn find_file_by_id(root: &Path, id: &str, extensions: &[&str]) -> Result<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        if name.contains(id) && extensions.iter().any(|candidate| candidate == &ext) {
            return Ok(Some(path.to_path_buf()));
        }
    }
    Ok(None)
}

fn print_inspect_header(
    transcript: &Transcript,
    turns: &[&TranscriptTurn],
    options: &InspectOptions,
) {
    let cwd = transcript
        .session
        .cwd
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let mode = if let Some(find) = &options.find {
        format!("find={find}")
    } else if options.timeline {
        "timeline".to_string()
    } else {
        format!("role={}", role_filter_label(options.role))
    };
    println!(
        "Source: {}:{} · {} · showing {} of {} turns",
        transcript.session.provider,
        transcript.session.id,
        mode,
        turns.len(),
        transcript.turns.len()
    );
    println!("Title: {}", transcript.session.title);
    println!("Project: {cwd}\n");
}

fn print_turn_preview(turn: &TranscriptTurn, expand: bool) {
    let ts = turn
        .timestamp
        .map(|ts| ts.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string());
    if expand {
        println!("[{:03}] {} {}", turn.turn, ts, turn.role.label());
        print_expanded_turn(turn, "  ");
        println!();
    } else {
        let preview = truncate(&turn.preview_text(), 220);
        println!(
            "[{:03}] {} {:<6} {}",
            turn.turn,
            ts,
            turn.role.label(),
            preview
        );
    }
}

fn print_expanded_turn(turn: &TranscriptTurn, indent: &str) {
    if turn.role == TranscriptRole::Tool {
        if let Some(tool) = &turn.tool_name {
            println!("{indent}tool: {tool}");
        }
    }
    if let Some(input) = &turn.tool_input {
        println!("{indent}input: {}", truncate(input, 900));
    }
    if !turn.text.trim().is_empty() {
        println!("{indent}{}", truncate(&turn.text, 1200));
    }
    if let Some(result) = &turn.tool_result {
        println!("{indent}result: {}", truncate(result, 900));
    }
}

fn render_context_markdown(
    transcript: &Transcript,
    turns: &[&TranscriptTurn],
    options: &ContextOptions,
) -> String {
    let cwd = transcript
        .session
        .cwd
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let source_path = transcript
        .session
        .transcript_path
        .as_ref()
        .or(transcript.session.source_path.as_ref())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let mut out = String::new();
    out.push_str("## Source\n");
    out.push_str(&format!(
        "Provider: {}\nID: {}\nTitle: {}\nProject: {}\nSource path: {}\nResume command: {}\n\n",
        transcript.session.provider,
        transcript.session.id,
        transcript.session.title,
        cwd,
        source_path,
        transcript.session.resume_command
    ));
    out.push_str("## Budget\n");
    out.push_str(&format!(
        "Turns: {} of {}\nEstimated tokens: ~{}\nTool policy: {}\n\n",
        turns.len(),
        transcript.turns.len(),
        estimated_tokens(turns),
        if options.no_tools {
            "omitted"
        } else if options.truncate_tools {
            "truncated"
        } else {
            "included"
        }
    ));
    out.push_str("## Transcript\n\n");
    for turn in clean_context_turns(turns, options) {
        let ts = turn
            .timestamp
            .map(|ts| ts.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "[{:03}] {} {}\n",
            turn.turn,
            ts,
            turn.role.label()
        ));
        if turn.role == TranscriptRole::Tool {
            if let Some(tool) = &turn.tool_name {
                out.push_str(&format!("tool: {tool}\n"));
            }
        }
        if let Some(input) = &turn.tool_input {
            out.push_str(&format!("input: {}\n", truncate(input, 1200)));
        }
        if !turn.text.trim().is_empty() {
            out.push_str(&format!("{}\n", truncate(&turn.text, 2400)));
        }
        if let Some(result) = &turn.tool_result {
            out.push_str(&format!("result: {}\n", truncate(result, 1200)));
        }
        out.push('\n');
    }
    out
}

fn print_context_preflight(
    transcript: &Transcript,
    options: &ContextOptions,
    selected_turns: usize,
    estimated: usize,
    max_tokens: usize,
) {
    println!("This context view is large.\n");
    println!(
        "Conversation: {}:{}",
        transcript.session.provider, transcript.session.id
    );
    println!("Title: {}", transcript.session.title);
    println!(
        "Selected turns: {selected_turns} of {}",
        transcript.turns.len()
    );
    println!("Estimated output: ~{estimated} tokens");
    println!("Current budget: {max_tokens} tokens\n");
    println!("Recommended:");
    println!("- Use --from-turn/--to-turn or --around/--context to inspect a smaller range.");
    println!(
        "- Use fainder inspect {}:{} --role user to locate the relevant area.",
        transcript.session.provider, transcript.session.id
    );
    println!("- Use --confirm to print anyway.\n");
    println!("Run:");
    println!("fainder context {} --confirm", options.session_ref);
}

fn clean_context_turns(turns: &[&TranscriptTurn], options: &ContextOptions) -> Vec<TranscriptTurn> {
    turns
        .iter()
        .map(|turn| {
            let mut next = (*turn).clone();
            if options.truncate_tools && next.role == TranscriptRole::Tool {
                next.text = truncate(&next.text, 500);
                next.tool_input = next.tool_input.map(|value| truncate(&value, 500));
                next.tool_result = next.tool_result.map(|value| truncate(&value, 500));
            }
            next
        })
        .collect()
}

/// Per-turn character cap for `inspect --json` previews (without --expand).
const INSPECT_JSON_PREVIEW_CHARS: usize = 280;

/// Clone a turn, truncating its text/tool bodies unless `expand` is set.
fn bound_turn(turn: &TranscriptTurn, expand: bool, max_chars: usize) -> TranscriptTurn {
    let mut next = turn.clone();
    if !expand {
        next.text = truncate(&next.text, max_chars);
        next.tool_input = next.tool_input.map(|value| truncate(&value, max_chars));
        next.tool_result = next.tool_result.map(|value| truncate(&value, max_chars));
    }
    next
}

#[derive(Serialize)]
struct InspectOutput {
    provider: ProviderKind,
    id: String,
    title: String,
    cwd: Option<String>,
    total_turns: usize,
    /// True when turn bodies are previews; pass --expand for full text.
    truncated: bool,
    turns: Vec<TranscriptTurn>,
}

#[derive(Serialize)]
struct ContextOutput {
    provider: ProviderKind,
    id: String,
    title: String,
    cwd: Option<String>,
    source_path: Option<String>,
    resume_command: String,
    total_turns: usize,
    estimated_tokens: usize,
    turns: Vec<TranscriptTurn>,
}

trait TurnText {
    fn search_text(&self) -> String;
    fn preview_text(&self) -> String;
}

impl TurnText for TranscriptTurn {
    fn search_text(&self) -> String {
        [
            self.text.as_str(),
            self.tool_name.as_deref().unwrap_or_default(),
            self.tool_input.as_deref().unwrap_or_default(),
            self.tool_result.as_deref().unwrap_or_default(),
        ]
        .join(" ")
    }

    fn preview_text(&self) -> String {
        if self.role == TranscriptRole::Tool {
            let mut parts = Vec::new();
            if let Some(tool) = &self.tool_name {
                parts.push(tool.clone());
            }
            if let Some(input) = &self.tool_input {
                parts.push(input.clone());
            }
            if let Some(result) = &self.tool_result {
                parts.push(format!("result: {result}"));
            }
            if parts.is_empty() {
                self.text.clone()
            } else {
                parts.join(" · ")
            }
        } else {
            self.text.clone()
        }
    }
}

struct TextMatcher {
    regex: Option<Regex>,
    terms: Vec<String>,
}

impl TextMatcher {
    fn new(pattern: &str, regex: bool) -> Result<Self> {
        if regex {
            Ok(Self {
                regex: Some(RegexBuilder::new(pattern).case_insensitive(true).build()?),
                terms: Vec::new(),
            })
        } else {
            Ok(Self {
                regex: None,
                terms: pattern
                    .split_whitespace()
                    .map(|part| part.to_lowercase())
                    .collect(),
            })
        }
    }

    fn is_match(&self, text: &str) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(text);
        }
        let text = text.to_lowercase();
        self.terms.iter().all(|term| text.contains(term))
    }
}

fn window(turns: &[TranscriptTurn], center: usize, context: usize) -> Vec<&TranscriptTurn> {
    let start = center.saturating_sub(context).max(1);
    let end = center.saturating_add(context);
    turns
        .iter()
        .filter(|turn| turn.turn >= start && turn.turn <= end)
        .collect()
}

fn take_tail<T>(items: Vec<T>, tail: usize) -> Vec<T> {
    let len = items.len();
    items.into_iter().skip(len.saturating_sub(tail)).collect()
}

fn take_tail_refs(turns: &[TranscriptTurn], tail: usize) -> Vec<&TranscriptTurn> {
    turns
        .iter()
        .skip(turns.len().saturating_sub(tail))
        .collect()
}

fn estimated_tokens(turns: &[&TranscriptTurn]) -> usize {
    estimate_tokens(
        turns
            .iter()
            .map(|turn| turn.search_text().len())
            .sum::<usize>(),
    )
}

fn estimate_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

fn safe_boundary(value: &str, max: usize) -> usize {
    let mut boundary = max.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn role_filter_label(role: RoleFilter) -> &'static str {
    match role {
        RoleFilter::User => "user",
        RoleFilter::Agent => "agent",
        RoleFilter::Tool => "tool",
        RoleFilter::System => "system",
        RoleFilter::All => "all",
    }
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    string_field(value, "timestamp")
        .or_else(|| string_field(value, "time"))
        .and_then(|value| parse_time(Some(&value)))
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

fn millis(value: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(value).single()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

fn infer_role_from_value(value: &Value) -> TranscriptRole {
    let role_text = value
        .get("role")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if role_text.contains("user") {
        TranscriptRole::User
    } else if role_text.contains("assistant") || role_text.contains("agent") {
        TranscriptRole::Agent
    } else if role_text.contains("tool") {
        TranscriptRole::Tool
    } else {
        TranscriptRole::Unknown
    }
}

fn contains_tool_result(value: &Value) -> bool {
    match value {
        Value::Array(items) => items
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_result")),
        Value::Object(map) => map.get("type").and_then(Value::as_str) == Some("tool_result"),
        _ => false,
    }
}

fn is_local_command_noise(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<command-name>")
        || text.starts_with("<local-command-stdout>")
        || text.starts_with("<local-command-caveat>")
}

fn text_from_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(text_from_value)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(map) => {
            if let Some(text) = map
                .get("text")
                .or_else(|| map.get("message"))
                .or_else(|| map.get("output"))
                .or_else(|| map.get("content"))
                .or_else(|| map.get("input"))
                .or_else(|| map.get("arguments"))
                .or_else(|| map.get("data"))
            {
                return text_from_value(text);
            }
            map.values()
                .take(8)
                .map(text_from_value)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => String::new(),
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let mut out = chars[..max_chars].iter().collect::<String>();
    out.push('…');
    out
}

fn shell(value: &str) -> String {
    escape(value.into()).to_string()
}
