use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use shell_escape::escape;
use walkdir::WalkDir;

use crate::config::Config;
use crate::model::{ProviderKind, ProviderReport, Session};
use crate::search::{Matcher, highlightless_snippet};

pub fn sessions(config: &Config, provider: ProviderKind) -> Result<Vec<Session>> {
    match provider {
        ProviderKind::Codex => codex_sessions_all(&config.roots(provider)),
        ProviderKind::Claude => claude_sessions(&config.path(provider)),
        ProviderKind::Opencode => opencode_sessions(&config.path(provider)),
        ProviderKind::Hermes => hermes_sessions(&config.path(provider)),
        ProviderKind::Cursor => vscode_style_sessions(&config.path(provider), provider, "cursor"),
        ProviderKind::Copilot => vscode_style_sessions(&config.path(provider), provider, "code"),
        ProviderKind::Kiro => kiro_sessions(&config.path(provider)),
    }
}

pub fn recent_sessions(
    config: &Config,
    provider: ProviderKind,
    limit: usize,
) -> Result<Vec<Session>> {
    let limit = limit.max(1);
    match provider {
        ProviderKind::Codex => {
            let mut sessions = Vec::new();
            for root in config.roots(provider) {
                let mut found = recent_file_sessions(
                    &root.join("sessions"),
                    limit,
                    |path| !skip_codex_path(path),
                    codex_session_from_transcript,
                )?;
                found.extend(codex_history_sessions(&root)?);
                sessions.extend(with_codex_home(found, &root));
            }
            let mut sessions = merged_sessions(sessions);
            sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
            sessions.truncate(limit);
            Ok(sessions)
        }
        ProviderKind::Claude => recent_file_sessions(
            &config.path(provider).join("projects"),
            limit,
            |path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| !stem.starts_with("agent-"))
            },
            |path| {
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    Ok(None)
                } else {
                    claude_session_from_transcript(path, id)
                }
            },
        ),
        ProviderKind::Opencode
        | ProviderKind::Hermes
        | ProviderKind::Cursor
        | ProviderKind::Copilot
        | ProviderKind::Kiro => {
            let mut sessions = sessions(config, provider)?;
            sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            sessions.truncate(limit);
            Ok(sessions)
        }
    }
}

pub fn content_sessions(
    config: &Config,
    provider: ProviderKind,
    matcher: &Matcher,
    query: &str,
    limit: usize,
) -> Result<Vec<(Session, Vec<String>)>> {
    match provider {
        ProviderKind::Codex => {
            let mut results: Vec<(Session, Vec<String>)> = Vec::new();
            for root in config.roots(provider) {
                if results.len() >= limit {
                    break;
                }
                let remaining = limit - results.len();
                let mut found = file_content_sessions(
                    &root,
                    matcher,
                    query,
                    remaining,
                    codex_session_from_transcript,
                )?;
                if found.len() < remaining {
                    found.extend(codex_history_content_sessions(
                        &root,
                        matcher,
                        query,
                        remaining - found.len(),
                    )?);
                }
                for (session, snippets) in found {
                    let session = with_codex_home(vec![session], &root).remove(0);
                    if !results.iter().any(|(seen, _)| seen.id == session.id) {
                        results.push((session, snippets));
                    }
                }
            }
            results.truncate(limit);
            Ok(results)
        }
        ProviderKind::Claude => file_content_sessions(
            &config.path(provider).join("projects"),
            matcher,
            query,
            limit,
            |path| {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                if id.starts_with("agent-") {
                    Ok(None)
                } else {
                    claude_session_from_transcript(path, id)
                }
            },
        ),
        ProviderKind::Opencode => opencode_content_sessions(&config.path(provider), matcher, limit),
        ProviderKind::Hermes => file_content_sessions(
            &config.path(provider),
            matcher,
            query,
            limit,
            hermes_session_from_transcript,
        ),
        ProviderKind::Cursor | ProviderKind::Copilot => {
            vscode_style_content_sessions(config, provider, matcher, limit)
        }
        ProviderKind::Kiro => kiro_content_sessions(&config.path(provider), matcher, limit),
    }
}

fn recent_file_sessions<F, P>(
    root: &Path,
    limit: usize,
    include: P,
    parser: F,
) -> Result<Vec<Session>>
where
    F: Fn(&Path) -> Result<Option<Session>>,
    P: Fn(&Path) -> bool,
{
    let mut paths = jsonl_paths(root)
        .into_iter()
        .filter(|path| include(path))
        .collect::<Vec<_>>();
    paths.sort_by(|a, b| metadata_time(b).cmp(&metadata_time(a)));

    let mut sessions = Vec::new();
    for path in paths.into_iter().take(limit.saturating_mul(4)) {
        if let Some(session) = parser(&path)? {
            sessions.push(session);
            if sessions.len() >= limit {
                break;
            }
        }
    }
    Ok(sessions)
}

pub fn doctor(config: &Config) -> Result<Vec<ProviderReport>> {
    let mut reports = Vec::new();
    for provider in ProviderKind::all() {
        let path = config.path(provider);
        let exists = path.exists();
        let (sessions, warnings) = match self::sessions(config, provider) {
            Ok(sessions) => (sessions.len(), Vec::new()),
            Err(error) => (0, vec![error.to_string()]),
        };
        reports.push(ProviderReport {
            provider,
            path,
            exists,
            sessions,
            warnings,
        });
    }
    Ok(reports)
}

fn codex_sessions_all(roots: &[PathBuf]) -> Result<Vec<Session>> {
    let mut sessions = Vec::new();
    for root in roots {
        sessions.extend(with_codex_home(codex_sessions(root)?, root));
    }
    Ok(merged_sessions(sessions))
}

fn codex_sessions(root: &Path) -> Result<Vec<Session>> {
    let mut sessions = Vec::new();
    let index = root.join("session_index.jsonl");
    if index.exists() {
        for value in read_jsonl(&index)? {
            let id = string_field(&value, "id").unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let title = string_field(&value, "thread_name").unwrap_or_else(|| id.clone());
            let updated_at = parse_time(string_field(&value, "updated_at").as_deref());
            sessions.push(Session {
                provider: ProviderKind::Codex,
                id: id.clone(),
                title,
                cwd: None,
                created_at: None,
                updated_at,
                message_count: None,
                source_path: Some(index.clone()),
                transcript_path: None,
                resume_command: format!("codex resume {}", shell(&id)),
                latest_messages: Vec::new(),
            });
        }
    }

    for path in json_files(&root.join("sessions"), true)
        .into_iter()
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter(|path| !skip_codex_path(path))
    {
        if let Some(session) = codex_session_from_transcript(&path)? {
            sessions.push(session);
        }
    }
    sessions.extend(codex_history_sessions(root)?);

    Ok(merged_sessions(sessions))
}

fn merged_sessions(sessions: Vec<Session>) -> Vec<Session> {
    let mut by_key: HashMap<(ProviderKind, String), Session> = HashMap::new();
    for session in sessions {
        let key = (session.provider, session.id.clone());
        match by_key.get_mut(&key) {
            Some(existing) => merge_session(existing, session),
            None => {
                by_key.insert(key, session);
            }
        }
    }
    by_key.into_values().collect()
}

fn merge_session(existing: &mut Session, next: Session) {
    if existing.title == existing.id && next.title != next.id {
        existing.title = next.title;
    }
    if existing.cwd.is_none() && next.cwd.is_some() {
        existing.cwd = next.cwd;
        existing.resume_command = next.resume_command;
    }
    if existing.created_at.is_none()
        || next
            .created_at
            .is_some_and(|created| existing.created_at.is_some_and(|current| created < current))
    {
        existing.created_at = next.created_at;
    }
    if existing.updated_at.is_none()
        || next
            .updated_at
            .is_some_and(|updated| existing.updated_at.is_some_and(|current| updated > current))
    {
        existing.updated_at = next.updated_at;
    }
    if existing.message_count.is_none() && next.message_count.is_some() {
        existing.message_count = next.message_count;
    }
    if existing.source_path.is_none() && next.source_path.is_some() {
        existing.source_path = next.source_path;
    }
    if existing.transcript_path.is_none() && next.transcript_path.is_some() {
        existing.transcript_path = next.transcript_path;
    }
    if existing.latest_messages.is_empty() && !next.latest_messages.is_empty() {
        existing.latest_messages = next.latest_messages;
    }
}

fn codex_message_text(value: &Value) -> Option<(bool, String)> {
    match value.get("type").and_then(Value::as_str) {
        Some("event_msg") => {
            let payload = value.get("payload")?;
            let is_user_message =
                payload.get("type").and_then(Value::as_str) == Some("user_message");
            string_field(payload, "message")
                .filter(|message| !message.trim().is_empty())
                .map(|message| (is_user_message, message))
        }
        Some("response_item") => {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(Value::as_str) != Some("message") {
                return None;
            }
            let is_user_message = payload.get("role").and_then(Value::as_str) == Some("user");
            let text = text_from_value(payload.get("content").unwrap_or(payload));
            (!text.trim().is_empty()).then_some((is_user_message, text))
        }
        _ => None,
    }
}

fn timestamp_from_value(value: &Value) -> Option<DateTime<Utc>> {
    string_field(value, "timestamp")
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| string_field(payload, "timestamp"))
        })
        .and_then(|timestamp| parse_time(Some(&timestamp)))
}

fn metadata_time(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(DateTime::<Utc>::from)
}

fn jsonl_paths(root: &Path) -> Vec<PathBuf> {
    json_files(root, true)
        .into_iter()
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect()
}

fn claude_transcript_paths(projects: &Path) -> Vec<PathBuf> {
    jsonl_paths(projects)
        .into_iter()
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| !stem.starts_with("agent-"))
        })
        .collect()
}

fn max_time(current: Option<DateTime<Utc>>, next: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Some(current.map_or(next, |current| current.max(next)))
}

fn min_time(current: Option<DateTime<Utc>>, next: DateTime<Utc>) -> Option<DateTime<Utc>> {
    Some(current.map_or(next, |current| current.min(next)))
}

fn codex_session_from_transcript(path: &Path) -> Result<Option<Session>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut id = None;
    let mut cwd = None;
    let mut title = None;
    let mut created_at = None;
    let mut updated_at = metadata_time(path);
    let mut message_count = 0usize;
    let mut latest = Vec::new();
    let mut is_subagent = false;

    for line in reader.lines().map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(ts) = timestamp_from_value(&value) {
            updated_at = max_time(updated_at, ts);
            created_at = min_time(created_at, ts);
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = value.get("payload") {
                id = string_field(payload, "id").or(id);
                cwd = string_field(payload, "cwd").map(PathBuf::from).or(cwd);
                is_subagent |= payload
                    .get("source")
                    .and_then(|source| source.get("subagent"))
                    .is_some()
                    || payload.get("thread_source").and_then(Value::as_str) == Some("subagent");
            }
        }
        if let Some((is_user_message, message)) = codex_message_text(&value) {
            if is_context_message(&message) {
                continue;
            }
            message_count += 1;
            if title.is_none() && is_user_message {
                title = Some(short_title(&message));
            }
            if is_user_message {
                push_latest(&mut latest, message);
            }
        }
    }

    let id = id.or_else(|| id_from_codex_filename(path));
    let Some(id) = id else {
        return Ok(None);
    };
    if is_subagent {
        return Ok(None);
    }
    let title = title.unwrap_or_else(|| id.clone());
    let resume_command = if let Some(cwd) = &cwd {
        format!("cd {} && codex resume {}", shell_path(cwd), shell(&id))
    } else {
        format!("codex resume {}", shell(&id))
    };

    Ok(Some(Session {
        provider: ProviderKind::Codex,
        id,
        title,
        cwd,
        created_at,
        updated_at,
        message_count: Some(message_count),
        source_path: Some(path.to_path_buf()),
        transcript_path: Some(path.to_path_buf()),
        resume_command,
        latest_messages: latest,
    }))
}

fn claude_sessions(root: &Path) -> Result<Vec<Session>> {
    let projects = root.join("projects");
    let mut sessions = Vec::new();
    if !projects.exists() {
        return Ok(sessions);
    }

    for path in WalkDir::new(&projects)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("sessions-index.json"))
    {
        let raw = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&raw)?;
        let entries = value
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in entries {
            let id = string_field(&entry, "sessionId").unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let title = string_field(&entry, "summary")
                .or_else(|| string_field(&entry, "firstPrompt").map(|t| short_title(&t)))
                .unwrap_or_else(|| id.clone());
            let transcript_path = string_field(&entry, "fullPath").map(PathBuf::from);
            let cwd = string_field(&entry, "projectPath").map(PathBuf::from);
            let latest = string_field(&entry, "firstPrompt").into_iter().collect();
            let resume_command = if let Some(cwd) = &cwd {
                format!("cd {} && claude --resume {}", shell_path(cwd), shell(&id))
            } else {
                format!("claude --resume {}", shell(&id))
            };
            sessions.push(Session {
                provider: ProviderKind::Claude,
                id,
                title,
                cwd,
                created_at: parse_time(string_field(&entry, "created").as_deref()),
                updated_at: parse_time(string_field(&entry, "modified").as_deref()),
                message_count: int_field(&entry, "messageCount").map(|count| count as usize),
                source_path: Some(path.clone()),
                transcript_path,
                resume_command,
                latest_messages: latest,
            });
        }
    }

    for path in claude_transcript_paths(&projects) {
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        if let Some(session) = claude_session_from_transcript(&path, id)? {
            sessions.push(session);
        }
    }

    Ok(merged_sessions(sessions))
}

fn claude_session_from_transcript(path: &Path, id: String) -> Result<Option<Session>> {
    let mut title = None;
    let mut cwd = None;
    let mut created_at = None;
    let mut updated_at = metadata_time(path);
    let mut message_count = 0usize;
    let mut latest = Vec::new();

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        if let Some(ts) = timestamp_from_value(&value) {
            updated_at = max_time(updated_at, ts);
            created_at = min_time(created_at, ts);
        }
        cwd = string_field(&value, "cwd").map(PathBuf::from).or(cwd);
        let message_type = value.get("type").and_then(Value::as_str);
        if matches!(message_type, Some("user") | Some("assistant")) {
            if let Some(message) = value.get("message") {
                let text = text_from_value(message.get("content").unwrap_or(message));
                if !text.is_empty() {
                    if is_context_message(&text) {
                        continue;
                    }
                    message_count += 1;
                    if message_type == Some("user") {
                        title.get_or_insert_with(|| short_title(&text));
                        push_latest(&mut latest, text);
                    }
                }
            }
        }
    }

    let title = title.unwrap_or_else(|| id.clone());
    let resume_command = if let Some(cwd) = &cwd {
        format!("cd {} && claude --resume {}", shell_path(cwd), shell(&id))
    } else {
        format!("claude --resume {}", shell(&id))
    };
    Ok(Some(Session {
        provider: ProviderKind::Claude,
        id,
        title,
        cwd,
        created_at,
        updated_at,
        message_count: Some(message_count),
        source_path: Some(path.to_path_buf()),
        transcript_path: Some(path.to_path_buf()),
        resume_command,
        latest_messages: latest,
    }))
}

fn opencode_sessions(db_path: &Path) -> Result<Vec<Session>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare(
        "select id, title, directory, time_created, time_updated from session order by time_updated desc",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let directory: String = row.get(2)?;
        let created: i64 = row.get(3)?;
        let updated: i64 = row.get(4)?;
        let cwd = PathBuf::from(directory);
        Ok(Session {
            provider: ProviderKind::Opencode,
            id: id.clone(),
            title,
            cwd: Some(cwd.clone()),
            created_at: millis(created),
            updated_at: millis(updated),
            message_count: count_parts_for_session(db_path, &id).ok(),
            source_path: Some(db_path.to_path_buf()),
            transcript_path: Some(db_path.to_path_buf()),
            resume_command: format!(
                "cd {} && opencode --session {}",
                shell_path(&cwd),
                shell(&id)
            ),
            latest_messages: Vec::new(),
        })
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
}

fn hermes_sessions(root: &Path) -> Result<Vec<Session>> {
    let mut sessions = Vec::new();
    if !root.exists() {
        return Ok(sessions);
    }
    for path in fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
    {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("session_")
            || path.extension().and_then(|e| e.to_str()) != Some("json")
        {
            continue;
        }
        let raw = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let id = string_field(&value, "session_id")
            .or_else(|| {
                name.strip_prefix("session_")
                    .and_then(|n| n.strip_suffix(".json"))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| name.to_string());
        let title = string_field(&value, "title")
            .or_else(|| string_field(&value, "platform"))
            .unwrap_or_else(|| id.clone());
        let jsonl = path.with_file_name(format!("{}.jsonl", id));
        sessions.push(Session {
            provider: ProviderKind::Hermes,
            id: id.clone(),
            title,
            cwd: None,
            created_at: parse_time(string_field(&value, "session_start").as_deref()),
            updated_at: parse_time(string_field(&value, "last_updated").as_deref()),
            message_count: jsonl_message_count(&jsonl).ok(),
            source_path: Some(path),
            transcript_path: jsonl.exists().then_some(jsonl),
            resume_command: format!("hermes --resume {}", shell(&id)),
            latest_messages: Vec::new(),
        });
    }
    Ok(sessions)
}

fn hermes_session_from_transcript(path: &Path) -> Result<Option<Session>> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let id = stem.to_string();
    let sidecar = path.with_file_name(format!("session_{stem}.json"));
    if sidecar.exists() {
        let mut sessions = hermes_sessions(path.parent().unwrap_or_else(|| Path::new(".")))?;
        if let Some(mut session) = sessions
            .drain(..)
            .find(|session| session.id == id || session.transcript_path.as_deref() == Some(path))
        {
            session.transcript_path = Some(path.to_path_buf());
            return Ok(Some(session));
        }
    }
    Ok(Some(Session {
        provider: ProviderKind::Hermes,
        id: id.clone(),
        title: id.clone(),
        cwd: None,
        created_at: None,
        updated_at: fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from),
        message_count: jsonl_message_count(path).ok(),
        source_path: Some(path.to_path_buf()),
        transcript_path: Some(path.to_path_buf()),
        resume_command: format!("hermes --resume {}", shell(&id)),
        latest_messages: Vec::new(),
    }))
}

fn vscode_style_sessions(
    root: &Path,
    provider: ProviderKind,
    open_command: &str,
) -> Result<Vec<Session>> {
    let mut sessions = Vec::new();
    if !root.exists() {
        return Ok(sessions);
    }

    for db_path in WalkDir::new(root)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("state.vscdb"))
    {
        let workspace_dir = db_path.parent().unwrap_or(root);
        let cwd = workspace_path(workspace_dir);
        let mut workspace_sessions =
            vscode_sessions_from_db(&db_path, provider, open_command, cwd)?;
        sessions.append(&mut workspace_sessions);
    }

    Ok(sessions)
}

fn vscode_style_content_sessions(
    config: &Config,
    provider: ProviderKind,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<(Session, Vec<String>)>> {
    let open_command = match provider {
        ProviderKind::Cursor => "cursor",
        ProviderKind::Copilot => "code",
        _ => return Ok(Vec::new()),
    };
    let mut results = Vec::new();
    for session in vscode_style_sessions(&config.path(provider), provider, open_command)? {
        let snippets = vscode_content_snippets(&session, matcher, 3)?;
        if !snippets.is_empty() {
            results.push((session, snippets));
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results)
}

fn vscode_sessions_from_db(
    db_path: &Path,
    provider: ProviderKind,
    open_command: &str,
    cwd: Option<PathBuf>,
) -> Result<Vec<Session>> {
    let conn = Connection::open(db_path)?;
    let Ok(mut stmt) = conn.prepare(
        "select key, cast(value as text) from ItemTable \
         where key in ('composer.composerData', 'aiService.prompts', 'aiService.generations') \
            or key like '%chat%' \
            or key like '%copilot%' \
            or key like '%composer%'",
    ) else {
        return Ok(Vec::new());
    };

    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut composer_data = None;
    let mut prompts = Vec::new();
    let mut generations = Vec::new();
    let mut fallback_text = Vec::new();

    for row in rows.filter_map(|row| row.ok()) {
        let (key, value) = row;
        if key == "composer.composerData" {
            composer_data = serde_json::from_str::<Value>(&value).ok();
        } else if key == "aiService.prompts" {
            prompts = serde_json::from_str::<Vec<Value>>(&value).unwrap_or_default();
        } else if key == "aiService.generations" {
            generations = serde_json::from_str::<Vec<Value>>(&value).unwrap_or_default();
        } else {
            let text = searchable_line_text(&value);
            if !text.trim().is_empty() {
                fallback_text.push(text);
            }
        }
    }

    let mut sessions = Vec::new();
    if let Some(value) = composer_data {
        if let Some(composers) = value.get("allComposers").and_then(Value::as_array) {
            for composer in composers {
                let Some(id) = string_field(composer, "composerId") else {
                    continue;
                };
                let title = string_field(composer, "name")
                    .or_else(|| {
                        composer
                            .get("authoredPlan")
                            .and_then(|plan| string_field(plan, "title"))
                    })
                    .or_else(|| first_prompt_for_composer(&prompts, &id))
                    .unwrap_or_else(|| id.clone());
                let mut latest = Vec::new();
                if let Some(prompt) = first_prompt_for_composer(&prompts, &id) {
                    push_latest(&mut latest, prompt);
                }
                let created_at = int_field(composer, "createdAt").and_then(millis);
                let updated_at = int_field(composer, "lastUpdatedAt")
                    .and_then(millis)
                    .or_else(|| {
                        fs::metadata(db_path)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .map(DateTime::<Utc>::from)
                    });
                let message_count = composer_message_count(&prompts, &generations, &id);
                sessions.push(vscode_session(
                    provider,
                    id,
                    title,
                    cwd.clone(),
                    created_at,
                    updated_at,
                    message_count,
                    db_path,
                    open_command,
                    latest,
                ));
            }
        }
    }

    if sessions.is_empty()
        && (!prompts.is_empty() || !generations.is_empty() || !fallback_text.is_empty())
    {
        let id = db_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        let title = prompts
            .first()
            .and_then(|value| string_field(value, "text"))
            .or_else(|| {
                generations
                    .first()
                    .and_then(|value| string_field(value, "textDescription"))
            })
            .or_else(|| fallback_text.first().cloned())
            .map(|text| short_title(&text))
            .unwrap_or_else(|| id.clone());
        let mut latest = Vec::new();
        for value in prompts.iter().take(3) {
            if let Some(text) = string_field(value, "text") {
                push_latest(&mut latest, text);
            }
        }
        sessions.push(vscode_session(
            provider,
            id,
            title,
            cwd,
            None,
            fs::metadata(db_path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from),
            Some(prompts.len().max(generations.len())),
            db_path,
            open_command,
            latest,
        ));
    }

    Ok(sessions)
}

fn vscode_session(
    provider: ProviderKind,
    id: String,
    title: String,
    cwd: Option<PathBuf>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    message_count: Option<usize>,
    db_path: &Path,
    open_command: &str,
    latest_messages: Vec<String>,
) -> Session {
    let resume_command = if let Some(cwd) = &cwd {
        format!("{} {}", open_command, shell_path(cwd))
    } else {
        open_command.to_string()
    };
    Session {
        provider,
        id,
        title: short_title(&title),
        cwd,
        created_at,
        updated_at,
        message_count,
        source_path: Some(db_path.to_path_buf()),
        transcript_path: Some(db_path.to_path_buf()),
        resume_command,
        latest_messages,
    }
}

fn vscode_content_snippets(
    session: &Session,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(path) = &session.source_path else {
        return Ok(Vec::new());
    };
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare("select cast(value as text) from ItemTable")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut snippets = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        for text in json_text_chunks(&row) {
            if is_context_message(&text) {
                continue;
            }
            if matcher.is_match(&text) {
                snippets.push(highlightless_snippet(&text, ""));
                if snippets.len() >= limit {
                    return Ok(snippets);
                }
            }
        }
    }
    Ok(snippets)
}

// Kiro CLI (kiro.dev) persists conversations locally in a SQLite database at
// `~/Library/Application Support/kiro-cli/data.sqlite3`, confirmed via
// `.schema` on this machine (kiro-cli 2.21.0, unauthenticated so the tables
// are currently empty — no real conversation has been captured here).
//
//   conversations_v2(key, conversation_id, value, created_at, updated_at)
//   conversations(key, value)                      -- legacy v1 store
//
// `kiro-cli chat --resume` ("resume the most recent conversation from this
// directory"), `--all-cwds` ("span every workspace"), and `--session-source
// <v1|v2>` (which store to target) are documented CLI flags that make it
// safe to infer: `key` is the working directory a conversation belongs to,
// `conversation_id` is the session id, and `value` is a JSON blob holding
// the actual transcript. The exact shape of that blob is NOT confirmed
// (no logged-in session was available to inspect one), so parsing below is
// deliberately tolerant: it tries a likely `history` array of tagged
// messages first, then falls back to a generic scan for readable strings
// so a session still shows up instead of being skipped or crashing fainder.
fn kiro_sessions(db_path: &Path) -> Result<Vec<Session>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open(db_path)?;
    let mut sessions = kiro_sessions_v2(&conn, db_path)?;
    sessions.extend(kiro_sessions_v1(&conn, db_path)?);
    Ok(sessions)
}

fn kiro_sessions_v2(conn: &Connection, db_path: &Path) -> Result<Vec<Session>> {
    let Ok(mut stmt) = conn.prepare(
        "select key, conversation_id, value, created_at, updated_at from conversations_v2",
    ) else {
        // Table missing: an older/newer kiro-cli schema than the one we saw.
        return Ok(Vec::new());
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut sessions = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        let (key, conversation_id, value, created_at, updated_at) = row;
        sessions.push(kiro_row_to_session(
            key,
            Some(conversation_id),
            &value,
            millis(created_at),
            millis(updated_at),
            db_path,
        ));
    }
    Ok(sessions)
}

fn kiro_sessions_v1(conn: &Connection, db_path: &Path) -> Result<Vec<Session>> {
    let Ok(mut stmt) = conn.prepare("select key, value from conversations") else {
        return Ok(Vec::new());
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let fallback_time = metadata_time(db_path);
    let mut sessions = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        let (key, value) = row;
        sessions.push(kiro_row_to_session(
            key,
            None,
            &value,
            fallback_time,
            fallback_time,
            db_path,
        ));
    }
    Ok(sessions)
}

fn kiro_row_to_session(
    key: String,
    conversation_id: Option<String>,
    raw_value: &str,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    db_path: &Path,
) -> Session {
    let value: Value = serde_json::from_str(raw_value).unwrap_or(Value::Null);
    let messages = kiro_extract_messages(&value);
    // v2 rows carry a real session id kiro-cli can jump straight back to; v1
    // rows are one-per-directory with no id of their own, so the id we hand
    // out is just the key and resuming has to go through the directory-scoped
    // `--resume` instead of a (nonexistent) `--resume-id`.
    let is_v2 = conversation_id.is_some();
    let id = conversation_id.unwrap_or_else(|| key.clone());
    let cwd = kiro_cwd_from_key(&key);
    let mut title = kiro_declared_title(&value);
    let mut latest = Vec::new();
    let mut message_count = 0usize;
    for (is_user, text) in &messages {
        if is_context_message(text) {
            continue;
        }
        message_count += 1;
        if *is_user {
            title.get_or_insert_with(|| short_title(text));
            push_latest(&mut latest, text.clone());
        }
    }
    let title = title.unwrap_or_else(|| id.clone());
    let resume_command = match (&cwd, is_v2) {
        (Some(cwd), true) => format!(
            "cd {} && kiro-cli chat --resume-id {}",
            shell_path(cwd),
            shell(&id)
        ),
        (Some(cwd), false) => format!("cd {} && kiro-cli chat --resume", shell_path(cwd)),
        (None, true) => format!("kiro-cli chat --resume-id {}", shell(&id)),
        (None, false) => "kiro-cli chat --resume".to_string(),
    };
    Session {
        provider: ProviderKind::Kiro,
        id,
        title,
        cwd,
        created_at,
        updated_at,
        message_count: Some(message_count),
        source_path: Some(db_path.to_path_buf()),
        transcript_path: Some(db_path.to_path_buf()),
        resume_command,
        latest_messages: latest,
    }
}

fn kiro_declared_title(value: &Value) -> Option<String> {
    string_field(value, "title")
        .or_else(|| string_field(value, "summary"))
        .or_else(|| string_field(value, "name"))
}

/// `key` is the process cwd kiro-cli was started from, which the OS always
/// hands over already-expanded and absolute — a leading `~` never occurs in
/// practice, and treating it as one would break `cd` under `shell_path`'s
/// quoting (which disables tilde expansion).
fn kiro_cwd_from_key(key: &str) -> Option<PathBuf> {
    key.starts_with('/').then(|| PathBuf::from(key))
}

/// Best-effort extraction of `(is_user, text)` pairs from a stored Kiro
/// conversation blob.
///
/// Kiro CLI shares its Rust "codewhisperer" client stack with Amazon Q
/// Developer CLI (binary strings in `kiro-cli` reference `UserInputMessage`
/// and `AssistantResponseMessage`), whose local conversation state has
/// historically stored a `history` list where each turn is either a
/// single-key tagged object (serde's default externally-tagged enum
/// encoding, e.g. `{"UserInputMessage": {...}}`) or a `[user, assistant]`
/// pair — both shapes are handled by `kiro_collect_tagged_message`, which
/// recurses through arrays either way. No session was ever authenticated on
/// this machine to confirm the shape against a live payload, so a handful of
/// alternate top-level keys are tried too, and if none of them yield
/// anything a generic recursive text scan is used as a last resort — an
/// unrecognized schema should still produce something searchable instead of
/// a silently empty conversation.
fn kiro_extract_messages(value: &Value) -> Vec<(bool, String)> {
    for key in ["history", "messages", "transcript", "turns", "conversation"] {
        let Some(list) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        let mut out = Vec::new();
        for entry in list {
            kiro_collect_tagged_message(entry, &mut out);
        }
        if !out.is_empty() {
            return out;
        }
    }
    let mut chunks = Vec::new();
    collect_json_text(value, &mut chunks);
    chunks.into_iter().map(|text| (false, text)).collect()
}

fn kiro_collect_tagged_message(entry: &Value, out: &mut Vec<(bool, String)>) {
    match entry {
        Value::Object(map) if map.len() == 1 => {
            if let Some((tag, inner)) = map.iter().next() {
                let tag = tag.to_ascii_lowercase();
                let is_user = tag.contains("user") || tag.contains("prompt");
                let text = text_from_value(inner);
                if !text.trim().is_empty() {
                    out.push((is_user, text));
                }
            }
        }
        // `HistoryEntry { user, assistant, request_metadata }` — a real
        // 3-field struct confirmed via `strings` on the *kiro-cli-chat*
        // binary (`/Applications/Kiro CLI.app/Contents/MacOS/kiro-cli-chat`,
        // ~1.5GB — the chat engine, not the much smaller `kiro-cli` launcher
        // binary checked first, which doesn't contain it and led to an
        // earlier, mistaken "unconfirmed" edit of this comment). Debug-format
        // strings show the struct layout, not the exact serde JSON
        // attributes, but a plain derive with no rename serializes fields
        // under their Rust names — i.e. exactly this shape. One `history`
        // entry carries BOTH sides of an exchange. Without this arm the
        // generic branch below finds no `role`/`type` key, glues prompt and
        // response into one string, and labels all of it as the assistant.
        Value::Object(map) if map.contains_key("user") || map.contains_key("assistant") => {
            for (key, is_user) in [("user", true), ("assistant", false)] {
                let Some(inner) = map.get(key) else {
                    continue;
                };
                let text = text_from_value(inner);
                if !text.trim().is_empty() {
                    out.push((is_user, text));
                }
            }
        }
        Value::Object(_) => {
            let role = entry
                .get("role")
                .or_else(|| entry.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let is_user =
                role.contains("user") || role.contains("prompt") || role.contains("human");
            let text = text_from_value(entry);
            if !text.trim().is_empty() {
                out.push((is_user, text));
            }
        }
        Value::Array(items) => {
            for item in items {
                kiro_collect_tagged_message(item, out);
            }
        }
        _ => {}
    }
}

fn kiro_content_sessions(
    db_path: &Path,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<(Session, Vec<String>)>> {
    let mut results = Vec::new();
    for session in kiro_sessions(db_path)? {
        let snippets = kiro_content_snippets(&session, matcher, 3)?;
        if !snippets.is_empty() {
            results.push((session, snippets));
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results)
}

fn kiro_content_snippets(
    session: &Session,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(path) = &session.source_path else {
        return Ok(Vec::new());
    };
    let conn = Connection::open(path)?;
    let mut raw_values = Vec::new();
    if let Ok(mut stmt) =
        conn.prepare("select value from conversations_v2 where conversation_id = ?")
    {
        if let Ok(rows) = stmt.query_map([session.id.as_str()], |row| row.get::<_, String>(0)) {
            raw_values.extend(rows.filter_map(|row| row.ok()));
        }
    }
    if raw_values.is_empty() {
        if let Ok(mut stmt) = conn.prepare("select value from conversations where key = ?") {
            if let Ok(rows) = stmt.query_map([session.id.as_str()], |row| row.get::<_, String>(0)) {
                raw_values.extend(rows.filter_map(|row| row.ok()));
            }
        }
    }
    let mut snippets = Vec::new();
    for raw in raw_values {
        for text in json_text_chunks(&raw) {
            if is_context_message(&text) {
                continue;
            }
            if matcher.is_match(&text) {
                snippets.push(highlightless_snippet(&text, ""));
                if snippets.len() >= limit {
                    return Ok(snippets);
                }
            }
        }
    }
    Ok(snippets)
}

fn workspace_path(workspace_dir: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(workspace_dir.join("workspace.json")).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let uri = string_field(&value, "folder")
        .or_else(|| string_field(&value, "workspace"))
        .or_else(|| string_field(&value, "configuration"))?;
    file_uri_to_path(&uri)
}

fn file_uri_to_path(value: &str) -> Option<PathBuf> {
    if let Some(path) = value.strip_prefix("file://") {
        Some(PathBuf::from(percent_decode(path)))
    } else {
        Some(PathBuf::from(value))
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(hex);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn file_content_sessions<F>(
    root: &Path,
    matcher: &Matcher,
    query: &str,
    limit: usize,
    parser: F,
) -> Result<Vec<(Session, Vec<String>)>>
where
    F: Fn(&Path) -> Result<Option<Session>>,
{
    if !root.exists() {
        return Ok(Vec::new());
    }
    let paths = matching_paths(root, query, limit.saturating_mul(4).max(32));
    let mut results = Vec::new();
    for path in paths {
        if skip_codex_path(&path) {
            continue;
        }
        let Some(session) = parser(&path)? else {
            continue;
        };
        let snippets = file_content_snippets(&session, matcher, 3)?;
        if !snippets.is_empty() {
            results.push((session, snippets));
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results)
}

fn matching_paths(root: &Path, query: &str, limit: usize) -> Vec<PathBuf> {
    let needle = query.split_whitespace().next().unwrap_or(query);
    let output = Command::new("rg")
        .arg("--files-with-matches")
        .arg("--ignore-case")
        .arg("--follow")
        .arg("--glob")
        .arg("*.json")
        .arg("--glob")
        .arg("*.jsonl")
        .arg("--glob")
        .arg("!generated_images/**")
        .arg("--glob")
        .arg("!vendor_imports/**")
        .arg("--glob")
        .arg("!cache/**")
        .arg("--glob")
        .arg("!shell_snapshots/**")
        .arg("--glob")
        .arg("!ambient-suggestions/**")
        .arg("--")
        .arg(needle)
        .arg(root)
        .output();

    if let Ok(output) = output {
        if output.status.success() || output.status.code() == Some(1) {
            return String::from_utf8_lossy(&output.stdout)
                .lines()
                .take(limit)
                .map(PathBuf::from)
                .collect();
        }
    }

    json_files(root, true).into_iter().take(limit).collect()
}

fn opencode_content_sessions(
    db_path: &Path,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<(Session, Vec<String>)>> {
    let mut results = Vec::new();
    for session in opencode_sessions(db_path)? {
        let snippets = opencode_content_snippets(&session, matcher, 3)?;
        if !snippets.is_empty() {
            results.push((session, snippets));
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results)
}

fn file_content_snippets(
    session: &Session,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(path) = &session.transcript_path else {
        return Ok(Vec::new());
    };
    if !path.exists() || is_codex_history_db(path) {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut snippets = Vec::new();
    for line in reader.lines().map_while(|line| line.ok()) {
        let text = searchable_line_text(&line);
        if is_context_message(&text) {
            continue;
        }
        if matcher.is_match(&text) {
            snippets.push(highlightless_snippet(&text, ""));
            if snippets.len() >= limit {
                break;
            }
        }
    }
    Ok(snippets)
}

fn opencode_content_snippets(
    session: &Session,
    matcher: &Matcher,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(path) = &session.source_path else {
        return Ok(Vec::new());
    };
    let conn = Connection::open(path)?;
    let mut stmt =
        conn.prepare("select data from part where session_id = ? order by time_created desc")?;
    let rows = stmt.query_map([session.id.as_str()], |row| row.get::<_, String>(0))?;
    let mut snippets = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        let text = searchable_line_text(&row);
        if is_context_message(&text) {
            continue;
        }
        if matcher.is_match(&text) {
            snippets.push(highlightless_snippet(&text, ""));
            if snippets.len() >= limit {
                break;
            }
        }
    }
    Ok(snippets)
}

fn count_parts_for_session(db_path: &Path, session_id: &str) -> Result<usize> {
    let conn = Connection::open(db_path)?;
    let mut stmt = conn.prepare("select count(*) from part where session_id = ?")?;
    let count: i64 = stmt.query_row([session_id], |row| row.get(0))?;
    Ok(count.max(0) as usize)
}

fn jsonl_message_count(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut count = 0usize;
    for line in reader.lines().map_while(|line| line.ok()) {
        let text = searchable_line_text(&line);
        if !text.trim().is_empty() {
            count += 1;
        }
    }
    Ok(count)
}

fn composer_message_count(
    prompts: &[Value],
    generations: &[Value],
    composer_id: &str,
) -> Option<usize> {
    let prompt_count = prompts
        .iter()
        .filter(|value| {
            string_field(value, "composerId").is_none()
                || string_field(value, "composerId").as_deref() == Some(composer_id)
        })
        .count();
    let generation_count = generations
        .iter()
        .filter(|value| {
            string_field(value, "composerId").is_none()
                || string_field(value, "composerId").as_deref() == Some(composer_id)
        })
        .count();
    let total = prompt_count + generation_count;
    (total > 0).then_some(total)
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);
    Ok(reader
        .lines()
        .map_while(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect())
}

fn json_files(root: &Path, recursive: bool) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    // Follow links: Codex users move old `sessions/YYYY/MM` folders to
    // another disk and leave a symlink behind.
    let walker = if recursive {
        WalkDir::new(root).follow_links(true)
    } else {
        WalkDir::new(root).max_depth(1).follow_links(true)
    };
    walker
        .into_iter()
        .filter_map(|entry| entry.ok())
        .map(|e| e.into_path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("json") | Some("jsonl")
            )
        })
        .collect()
}

fn skip_codex_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/generated_images/")
        || text.contains("/vendor_imports/")
        || text.contains("/cache/")
        || text.contains("/shell_snapshots/")
        || text.contains("/ambient-suggestions/")
        || path.file_name().and_then(|n| n.to_str()) == Some("session_index.jsonl")
}

/// Codex 0.16x keeps thread metadata in `state_5.sqlite` and, for threads in
/// `paginated` history mode, a projection of every item in
/// `thread_history_1.sqlite`. Older rollout JSONL files get migrated there and
/// removed, so those threads only exist in the databases.
const CODEX_STATE_DB: &str = "state_5.sqlite";
const CODEX_HISTORY_DB: &str = "thread_history_1.sqlite";

pub fn is_codex_history_db(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(CODEX_HISTORY_DB)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

/// Sessions for threads whose rollout file no longer exists. Threads that
/// still have their JSONL are left to `codex_session_from_transcript`.
fn codex_history_sessions(root: &Path) -> Result<Vec<Session>> {
    let state = root.join(CODEX_STATE_DB);
    let history = root.join(CODEX_HISTORY_DB);
    if !state.exists() || !history.exists() {
        return Ok(Vec::new());
    }
    let conn = open_read_only(&state)?;
    let mut stmt = conn.prepare(
        "select id, rollout_path, created_at, updated_at, source, cwd, title, \
         first_user_message, name from threads",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut sessions = Vec::new();
    for row in rows.filter_map(|row| row.ok()) {
        let (id, rollout, created, updated, source, cwd, title, first_message, name) = row;
        // Same rule as the JSONL parser: subagents and guardian reviews stay out.
        if source.contains("\"subagent\"") || Path::new(&rollout).exists() {
            continue;
        }
        let title = [name.unwrap_or_default(), title, first_message]
            .into_iter()
            .find(|text| !text.trim().is_empty() && !is_context_message(text))
            .map(|text| short_title(&text))
            .unwrap_or_else(|| id.clone());
        let cwd = (!cwd.is_empty()).then(|| PathBuf::from(cwd));
        let resume_command = match &cwd {
            Some(cwd) => format!("cd {} && codex resume {}", shell_path(cwd), shell(&id)),
            None => format!("codex resume {}", shell(&id)),
        };
        sessions.push(Session {
            provider: ProviderKind::Codex,
            id,
            title,
            cwd,
            created_at: Utc.timestamp_opt(created, 0).single(),
            updated_at: Utc.timestamp_opt(updated, 0).single(),
            message_count: None,
            source_path: Some(state.clone()),
            transcript_path: Some(history.clone()),
            resume_command,
            latest_messages: Vec::new(),
        });
    }
    Ok(sessions)
}

/// User and agent messages of one thread from `thread_history_1.sqlite`, in
/// order, as `(created_at_ms, item_type, text)`.
pub fn codex_history_messages(
    history: &Path,
    thread_id: &str,
) -> Result<Vec<(i64, String, String)>> {
    let conn = open_read_only(history)?;
    let mut stmt = conn.prepare(
        "select created_at_ms, item_type, item_json from thread_items \
         where thread_id = ?1 order by rollout_ordinal",
    )?;
    let rows = stmt.query_map([thread_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    Ok(rows
        .filter_map(|row| row.ok())
        .filter_map(|(created, item_type, raw)| {
            let value = serde_json::from_str::<Value>(&raw).ok()?;
            let text = codex_history_item_text(&value)?;
            Some((created, item_type, text))
        })
        .collect())
}

pub fn codex_history_item_text(value: &Value) -> Option<String> {
    let text = match value.get("type").and_then(Value::as_str)? {
        "userMessage" => text_from_value(value.get("content")?),
        "agentMessage" => string_field(value, "text")?,
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn codex_history_content_sessions(
    root: &Path,
    matcher: &Matcher,
    query: &str,
    limit: usize,
) -> Result<Vec<(Session, Vec<String>)>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sessions = codex_history_sessions(root)?;
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let by_id: HashMap<&str, &Session> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let needle = query.split_whitespace().next().unwrap_or(query);
    let pattern = format!(
        "%{}%",
        needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let conn = open_read_only(&root.join(CODEX_HISTORY_DB))?;
    // One indexed lookup per migrated thread instead of a scan of every item:
    // threads that still have a JSONL were already searched with rg.
    let mut stmt = conn.prepare(
        "select item_json from thread_items \
         where thread_id = ?1 and item_json like ?2 escape '\\' \
         and item_type in ('userMessage', 'agentMessage') \
         order by rollout_ordinal",
    )?;
    let mut ordered = sessions.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    let mut order: Vec<String> = Vec::new();
    let mut snippets: HashMap<String, Vec<String>> = HashMap::new();
    for session in ordered {
        if order.len() >= limit {
            break;
        }
        let rows = stmt.query_map(rusqlite::params![session.id, pattern], |row| {
            row.get::<_, String>(0)
        })?;
        let mut found = Vec::new();
        for raw in rows.filter_map(|row| row.ok()) {
            let Some(text) = serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|value| codex_history_item_text(&value))
            else {
                continue;
            };
            if is_context_message(&text) || !matcher.is_match(&text) {
                continue;
            }
            found.push(highlightless_snippet(&text, ""));
            if found.len() >= 3 {
                break;
            }
        }
        if !found.is_empty() {
            order.push(session.id.clone());
            snippets.insert(session.id.clone(), found);
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| {
            let session = (*by_id.get(id.as_str())?).clone();
            let found = snippets.remove(&id)?;
            Some((session, found))
        })
        .collect())
}

/// Sessions from a Codex home other than `~/.codex` only resume with that
/// home selected, so the command carries `CODEX_HOME`.
fn with_codex_home(sessions: Vec<Session>, root: &Path) -> Vec<Session> {
    if root.file_name().and_then(|name| name.to_str()) == Some(".codex") {
        return sessions;
    }
    let prefix = format!("CODEX_HOME={} codex resume ", shell_path(root));
    sessions
        .into_iter()
        .map(|mut session| {
            if !session.resume_command.contains("CODEX_HOME=") {
                session.resume_command = session.resume_command.replace("codex resume ", &prefix);
            }
            session
        })
        .collect()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn int_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

fn first_prompt_for_composer(values: &[Value], composer_id: &str) -> Option<String> {
    values
        .iter()
        .find(|value| string_field(value, "composerId").as_deref() == Some(composer_id))
        .or_else(|| values.first())
        .and_then(|value| string_field(value, "text"))
        .map(|text| short_title(&text))
}

fn text_from_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(text_from_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(map) => map
            .get("message")
            .or_else(|| map.get("output"))
            .or_else(|| map.get("text"))
            .or_else(|| map.get("content"))
            .or_else(|| map.get("input_text"))
            .or_else(|| map.get("reasoning"))
            .or_else(|| map.get("description"))
            .map(text_from_value)
            .unwrap_or_else(|| {
                map.values()
                    .take(8)
                    .map(text_from_value)
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            }),
        _ => String::new(),
    }
}

fn json_text_chunks(raw: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return vec![searchable_line_text(raw)];
    };
    let mut chunks = Vec::new();
    collect_json_text(&value, &mut chunks);
    chunks
}

fn collect_json_text(value: &Value, chunks: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            if text.split_whitespace().count() >= 2 {
                chunks.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_json_text(item, chunks);
            }
        }
        Value::Object(map) => {
            for key in [
                "text",
                "textDescription",
                "response",
                "message",
                "content",
                "name",
                "title",
                "description",
            ] {
                if let Some(value) = map.get(key) {
                    collect_json_text(value, chunks);
                }
            }
            if chunks.len() < 64 {
                for value in map.values().take(16) {
                    collect_json_text(value, chunks);
                }
            }
        }
        _ => {}
    }
}

fn searchable_line_text(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .map(|value| text_from_value(&value))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| line.to_string())
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .ok()
}

fn millis(value: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(value).single()
}

fn id_from_codex_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    stem.rsplit('-').next().map(str::to_string)
}

fn short_title(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > 90 {
        format!("{}...", text.chars().take(87).collect::<String>())
    } else {
        text
    }
}

fn is_context_message(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("# AGENTS.md instructions")
        || text.starts_with("<environment_context>")
        || text.starts_with("<permissions instructions>")
        || text.starts_with("<user_instructions>")
        || text.starts_with("<image ")
        || text.starts_with("<command-message>")
        || text.starts_with("<command-name>")
        || text.starts_with("<local-command-caveat>")
        || text.starts_with("<local-command-stdout>")
        || text.starts_with("<local-command-stderr>")
        || text.starts_with("Base directory for this skill:")
        || text.starts_with("The following is the Codex agent history")
        || text.starts_with(">>> TRANSCRIPT START")
        || text.starts_with("Reviewed Codex session id:")
        || text
            .chars()
            .take(160)
            .collect::<String>()
            .contains("<command-message>")
}

fn push_latest(latest: &mut Vec<String>, text: String) {
    if text.trim().is_empty() {
        return;
    }
    latest.push(short_title(&text));
    if latest.len() > 5 {
        latest.remove(0);
    }
}

fn shell(value: &str) -> String {
    escape(value.into()).to_string()
}

fn shell_path(path: &Path) -> String {
    shell(&path.display().to_string())
}

#[cfg(test)]
mod kiro_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway SQLite file mirroring `kiro-cli`'s real schema
    /// (`conversations` / `conversations_v2`, confirmed via `.schema` on a
    /// logged-out install). Deleted on drop.
    struct TempDb {
        path: PathBuf,
    }

    impl TempDb {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("fainder-kiro-test-{name}-{nanos}.sqlite3"));
            Self { path }
        }

        fn seed(&self) -> Connection {
            let conn = Connection::open(&self.path).expect("open temp sqlite db");
            conn.execute_batch(
                "create table conversations (key text primary key, value text);
                 create table conversations_v2 (
                    key text not null,
                    conversation_id text not null,
                    value text not null,
                    created_at integer not null,
                    updated_at integer not null,
                    primary key (key, conversation_id)
                 );",
            )
            .expect("create kiro-cli schema");
            conn
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn parses_tagged_history_conversation() {
        let db = TempDb::new("history");
        let conn = db.seed();
        let value = serde_json::json!({
            "conversation_id": "conv-1",
            "history": [
                {"User": {"content": "revisa el deploy de smartorders"}},
                {"Assistant": {"content": "reviso los logs ahora"}},
                {"ToolUse": {"name": "fs_read", "input": {"path": "README.md"}}},
            ]
        })
        .to_string();
        conn.execute(
            "insert into conversations_v2 (key, conversation_id, value, created_at, updated_at) values (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "/Users/minisat/SmartUp/orders",
                "conv-1",
                value,
                1_700_000_000_000i64,
                1_700_000_100_000i64
            ],
        )
        .expect("insert v2 row");

        let sessions = kiro_sessions(&db.path).expect("read kiro sessions");
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.provider, ProviderKind::Kiro);
        assert_eq!(session.id, "conv-1");
        assert_eq!(
            session.cwd,
            Some(PathBuf::from("/Users/minisat/SmartUp/orders"))
        );
        assert_eq!(session.title, "revisa el deploy de smartorders");
        assert_eq!(session.message_count, Some(3));
        assert!(
            session
                .latest_messages
                .contains(&"revisa el deploy de smartorders".to_string())
        );
        assert!(session.resume_command.starts_with("cd "));
        assert!(
            session
                .resume_command
                .contains("kiro-cli chat --resume-id conv-1")
        );
    }

    #[test]
    fn falls_back_to_generic_text_scan_for_unrecognized_shape() {
        // A conversation blob that doesn't have a top-level "history" array
        // (an undocumented/future kiro-cli schema). The parser must still
        // surface something instead of dropping the session.
        let db = TempDb::new("fallback");
        let conn = db.seed();
        let value = serde_json::json!({
            "conversation_id": "conv-2",
            "messages": [
                {"speaker": "me", "body": "hola, como vamos con el pipeline"},
            ]
        })
        .to_string();
        conn.execute(
            "insert into conversations_v2 (key, conversation_id, value, created_at, updated_at) values (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "some-opaque-key",
                "conv-2",
                value,
                1_700_000_000_000i64,
                1_700_000_000_000i64
            ],
        )
        .expect("insert v2 row");

        let sessions = kiro_sessions(&db.path).expect("read kiro sessions");
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        // An opaque key isn't a path, so cwd stays unknown rather than guessed.
        assert_eq!(session.cwd, None);
        assert!(session.message_count.unwrap_or(0) > 0);
    }

    /// `HistoryEntry { user, assistant, request_metadata }`, confirmed via
    /// `strings` on the kiro-cli-chat binary. Both sides live in one entry,
    /// so they have to come out as two messages with the right sides, not
    /// one merged blob.
    #[test]
    fn splits_paired_user_assistant_history_entries() {
        let db = TempDb::new("paired");
        let conn = db.seed();
        let value = serde_json::json!({
            "conversation_id": "conv-paired",
            "history": [
                {
                    "user": {"content": "por que fallo el batch de smartvoc"},
                    "assistant": {"content": "se corto la transcripcion a la mitad"},
                    "request_metadata": {"request_id": "req-1"}
                }
            ]
        })
        .to_string();
        conn.execute(
            "insert into conversations_v2 (key, conversation_id, value, created_at, updated_at) values (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "/Users/minisat/SmartUp/smartvoc",
                "conv-paired",
                value,
                1_700_000_000_000i64,
                1_700_000_100_000i64
            ],
        )
        .expect("insert v2 row");

        let sessions = kiro_sessions(&db.path).expect("read kiro sessions");
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        // Two messages out of one entry, and the title comes from the user side
        // rather than from the assistant's answer or a merged string.
        assert_eq!(session.message_count, Some(2));
        assert_eq!(session.title, "por que fallo el batch de smartvoc");
        assert_eq!(
            session.latest_messages,
            vec!["por que fallo el batch de smartvoc".to_string()]
        );
    }

    #[test]
    fn reads_legacy_v1_conversations_table() {
        let db = TempDb::new("v1");
        let conn = db.seed();
        let value = serde_json::json!({
            "history": [
                {"User": {"content": "status del batch de smartvoc"}},
            ]
        })
        .to_string();
        conn.execute(
            "insert into conversations (key, value) values (?1, ?2)",
            rusqlite::params!["/Users/minisat/SmartUp/smartvoc", value],
        )
        .expect("insert v1 row");

        let sessions = kiro_sessions(&db.path).expect("read kiro sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "/Users/minisat/SmartUp/smartvoc");
        assert_eq!(
            sessions[0].cwd,
            Some(PathBuf::from("/Users/minisat/SmartUp/smartvoc"))
        );
    }

    #[test]
    fn missing_database_returns_empty_without_error() {
        let sessions =
            kiro_sessions(Path::new("/nonexistent/fainder-kiro-test/data.sqlite3")).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn content_search_matches_inside_conversation_json() {
        let db = TempDb::new("content-search");
        let conn = db.seed();
        let value = serde_json::json!({
            "conversation_id": "conv-3",
            "history": [
                {"User": {"content": "puedes revisar el webhook de kapso"}},
            ]
        })
        .to_string();
        conn.execute(
            "insert into conversations_v2 (key, conversation_id, value, created_at, updated_at) values (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                "/Users/minisat/SmartUp/multichannel",
                "conv-3",
                value,
                1_700_000_000_000i64,
                1_700_000_000_000i64
            ],
        )
        .expect("insert v2 row");
        drop(conn);

        let matcher = Matcher::new("kapso", crate::model::SearchMode::Phrase).unwrap();
        let results = kiro_content_sessions(&db.path, &matcher, 10).expect("content search");
        assert_eq!(results.len(), 1);
        assert!(!results[0].1.is_empty());
    }
}

#[cfg(test)]
mod codex_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway Codex home with the layout Codex CLI 0.16x writes:
    /// `sessions/YYYY/MM/DD/rollout-*.jsonl`, `state_5.sqlite` (thread
    /// metadata) and `thread_history_1.sqlite` (paginated item projection).
    /// Every value here is synthetic. Deleted on drop.
    struct TempHome {
        base: PathBuf,
        root: PathBuf,
    }

    impl TempHome {
        fn new(name: &str, home_dir: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos();
            let base = std::env::temp_dir().join(format!("fainder-codex-test-{name}-{nanos}"));
            let root = base.join(home_dir);
            fs::create_dir_all(root.join("sessions")).expect("create codex home");
            Self { base, root }
        }

        fn write_rollout(&self, day_dir: &Path, id: &str, source: Value, user: &str) -> PathBuf {
            fs::create_dir_all(day_dir).expect("create day dir");
            let path = day_dir.join(format!("rollout-2026-10-24T10-00-00-{id}.jsonl"));
            let lines = [
                serde_json::json!({
                    "timestamp": "2026-10-24T13:00:00.000Z",
                    "type": "session_meta",
                    "payload": {
                        "id": id,
                        "timestamp": "2026-10-24T13:00:00.000Z",
                        "cwd": "/tmp/fainder-fixture",
                        "originator": "codex_cli_rs",
                        "cli_version": "0.162.1",
                        "source": source,
                        "model_provider": "openai"
                    }
                }),
                serde_json::json!({
                    "timestamp": "2026-10-24T13:00:01.000Z",
                    "type": "response_item",
                    "payload": {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": user}]
                    }
                }),
                serde_json::json!({
                    "timestamp": "2026-10-24T13:00:01.000Z",
                    "type": "event_msg",
                    "payload": {"type": "user_message", "message": user, "images": []}
                }),
                serde_json::json!({
                    "timestamp": "2026-10-24T13:00:05.000Z",
                    "type": "event_msg",
                    "payload": {"type": "agent_message", "message": "listo, revisado"}
                }),
            ];
            let body = lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(&path, body).expect("write rollout");
            path
        }

        /// One row per thread: `(id, rollout_path, source, title)`, plus the
        /// user/agent items each thread has in the history projection.
        fn seed_databases(&self, threads: &[(&str, &Path, &str, &str)], items: &[(&str, Value)]) {
            let state = Connection::open(self.root.join(CODEX_STATE_DB)).expect("open state db");
            state
                .execute_batch(
                    "create table threads (
                        id text primary key,
                        rollout_path text not null,
                        created_at integer not null,
                        updated_at integer not null,
                        source text not null,
                        model_provider text not null,
                        cwd text not null,
                        title text not null,
                        first_user_message text not null default '',
                        name text,
                        history_mode text not null default 'legacy'
                    );",
                )
                .expect("create threads");
            for (id, rollout, source, title) in threads {
                state
                    .execute(
                        "insert into threads (id, rollout_path, created_at, updated_at, source, \
                         model_provider, cwd, title, first_user_message, history_mode) \
                         values (?1, ?2, 1760000000, 1760000100, ?3, 'openai', \
                         '/tmp/fainder-fixture', ?4, ?4, 'paginated')",
                        rusqlite::params![id, rollout.to_string_lossy(), source, title],
                    )
                    .expect("insert thread");
            }
            let history =
                Connection::open(self.root.join(CODEX_HISTORY_DB)).expect("open history db");
            history
                .execute_batch(
                    "create table thread_items (
                        thread_id text not null,
                        turn_id text not null,
                        item_id text not null,
                        rollout_ordinal integer not null,
                        created_at_ms integer not null,
                        item_json text not null,
                        item_type text not null default '',
                        primary key (thread_id, turn_id, item_id)
                    );",
                )
                .expect("create thread_items");
            for (ordinal, (thread_id, item)) in items.iter().enumerate() {
                history
                    .execute(
                        "insert into thread_items (thread_id, turn_id, item_id, rollout_ordinal, \
                         created_at_ms, item_json, item_type) values (?1, 'turn-1', ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            thread_id,
                            format!("item-{ordinal}"),
                            ordinal as i64,
                            1_760_000_000_000i64 + ordinal as i64,
                            item.to_string(),
                            item.get("type").and_then(Value::as_str).unwrap_or_default()
                        ],
                    )
                    .expect("insert item");
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn user_item(text: &str) -> Value {
        serde_json::json!({
            "type": "userMessage",
            "id": "item-u",
            "clientId": null,
            "content": [{"type": "text", "text": text, "text_elements": []}]
        })
    }

    fn agent_item(text: &str) -> Value {
        serde_json::json!({"type": "agentMessage", "id": "item-a", "phase": null, "text": text})
    }

    const LIVE: &str = "019a0000-0000-7000-8000-000000000001";
    const MIGRATED: &str = "019a0000-0000-7000-8000-000000000002";
    const GUARDIAN: &str = "019a0000-0000-7000-8000-000000000003";

    fn fixture(home_dir: &str) -> TempHome {
        let home = TempHome::new("fixture", home_dir);
        let day = home.root.join("sessions/2026/10/24");
        let live = home.write_rollout(
            &day,
            LIVE,
            Value::String("cli".to_string()),
            "revisa el webhook de kapso",
        );
        home.write_rollout(
            &day,
            "019a0000-0000-7000-8000-000000000009",
            serde_json::json!({"subagent": {"other": "guardian"}}),
            "aprueba este comando",
        );
        let gone = home.root.join("sessions/2026/10/02/rollout-gone.jsonl");
        home.seed_databases(
            &[
                (LIVE, &live, "cli", "revisa el webhook de kapso"),
                (MIGRATED, &gone, "exec", "audita el pipeline de smartvoc"),
                (
                    GUARDIAN,
                    &gone,
                    "{\"subagent\":{\"other\":\"guardian\"}}",
                    "guardian",
                ),
            ],
            &[
                (LIVE, user_item("revisa el webhook de kapso")),
                (MIGRATED, user_item("audita el pipeline de smartvoc")),
                (MIGRATED, agent_item("encontre dos batches con timeout")),
                (GUARDIAN, user_item("timeout en el guardian")),
            ],
        );
        home
    }

    #[test]
    fn lists_jsonl_and_migrated_sqlite_threads_once() {
        let home = fixture(".codex");
        let sessions = codex_sessions(&home.root).expect("read codex home");
        let mut ids = sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>();
        ids.sort();
        // The guardian JSONL and the guardian thread stay out; the live thread
        // appears once even though it is both a file and a database row.
        assert_eq!(ids, vec![LIVE, MIGRATED]);
        let migrated = sessions.iter().find(|s| s.id == MIGRATED).unwrap();
        assert_eq!(migrated.title, "audita el pipeline de smartvoc");
        assert!(
            migrated
                .transcript_path
                .as_deref()
                .is_some_and(is_codex_history_db)
        );
        assert_eq!(migrated.cwd, Some(PathBuf::from("/tmp/fainder-fixture")));
    }

    #[test]
    fn content_search_reads_threads_that_only_live_in_sqlite() {
        let home = fixture(".codex");
        let matcher = Matcher::new("timeout", crate::model::SearchMode::Phrase).unwrap();
        let results =
            codex_history_content_sessions(&home.root, &matcher, "timeout", 10).expect("search");
        // The guardian thread also says "timeout" but is not a session.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, MIGRATED);
        assert_eq!(results[0].1.len(), 1);
    }

    #[test]
    fn history_messages_come_back_in_order_with_roles() {
        let home = fixture(".codex");
        let messages = codex_history_messages(&home.root.join(CODEX_HISTORY_DB), MIGRATED)
            .expect("read history");
        let shape = messages
            .iter()
            .map(|(_, kind, text)| (kind.as_str(), text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            vec![
                ("userMessage", "audita el pipeline de smartvoc"),
                ("agentMessage", "encontre dos batches con timeout"),
            ]
        );
    }

    #[test]
    fn second_codex_home_resumes_with_its_codex_home() {
        let home = fixture(".codex-orq");
        let sessions =
            codex_sessions_all(std::slice::from_ref(&home.root)).expect("read second home");
        assert_eq!(sessions.len(), 2);
        for session in &sessions {
            assert!(
                session.resume_command.contains(&format!(
                    "CODEX_HOME={} codex resume ",
                    shell_path(&home.root)
                )),
                "{}",
                session.resume_command
            );
        }

        let default = fixture(".codex");
        let sessions =
            codex_sessions_all(std::slice::from_ref(&default.root)).expect("read default home");
        assert!(
            sessions
                .iter()
                .all(|s| !s.resume_command.contains("CODEX_HOME="))
        );
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_month_folders() {
        let home = TempHome::new("symlink", ".codex");
        let elsewhere = home.base.join("other-disk/2026/07");
        home.write_rollout(
            &elsewhere.join("15"),
            LIVE,
            Value::String("cli".to_string()),
            "migra el cluster de julio",
        );
        fs::create_dir_all(home.root.join("sessions/2026")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.root.join("sessions/2026/07")).unwrap();

        let sessions = codex_sessions(&home.root).expect("read codex home");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "migra el cluster de julio");

        let matcher = Matcher::new("cluster", crate::model::SearchMode::Phrase).unwrap();
        let found = file_content_sessions(
            &home.root,
            &matcher,
            "cluster",
            10,
            codex_session_from_transcript,
        )
        .expect("content search");
        assert_eq!(found.len(), 1);
    }
}
