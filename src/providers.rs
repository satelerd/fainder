use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::Connection;
use serde_json::Value;
use shell_escape::escape;
use walkdir::WalkDir;

use crate::config::Config;
use crate::model::{ProviderKind, ProviderReport, Session};
use crate::search::{Matcher, highlightless_snippet};

pub fn sessions(config: &Config, provider: ProviderKind) -> Result<Vec<Session>> {
    match provider {
        ProviderKind::Codex => codex_sessions(&config.path(provider)),
        ProviderKind::Claude => claude_sessions(&config.path(provider)),
        ProviderKind::Opencode => opencode_sessions(&config.path(provider)),
        ProviderKind::Hermes => hermes_sessions(&config.path(provider)),
        ProviderKind::Cursor => vscode_style_sessions(&config.path(provider), provider, "cursor"),
        ProviderKind::Copilot => vscode_style_sessions(&config.path(provider), provider, "code"),
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
        ProviderKind::Codex => file_content_sessions(
            &config.path(provider),
            matcher,
            query,
            limit,
            codex_session_from_transcript,
        ),
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
    }
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

    Ok(sessions)
}

fn codex_session_from_transcript(path: &Path) -> Result<Option<Session>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut id = None;
    let mut cwd = None;
    let mut title = None;
    let mut created_at = None;
    let mut updated_at = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);
    let mut message_count = 0usize;
    let mut latest = Vec::new();

    for line in reader.lines().map_while(|line| line.ok()).take(140) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(ts) = string_field(&value, "timestamp").and_then(|t| parse_time(Some(&t))) {
            updated_at = Some(ts);
            created_at.get_or_insert(ts);
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(payload) = value.get("payload") {
                id = string_field(payload, "id").or(id);
                cwd = string_field(payload, "cwd").map(PathBuf::from).or(cwd);
            }
        }
        if value.get("type").and_then(Value::as_str) == Some("event_msg") {
            if let Some(payload) = value.get("payload") {
                if let Some(message) = string_field(payload, "message") {
                    message_count += 1;
                    let is_user_message =
                        payload.get("type").and_then(Value::as_str) == Some("user_message");
                    if title.is_none() && is_user_message {
                        title = Some(short_title(&message));
                    }
                    if is_user_message {
                        push_latest(&mut latest, message);
                    }
                }
            }
        }
    }

    let id = id.or_else(|| id_from_codex_filename(path));
    let Some(id) = id else {
        return Ok(None);
    };
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

    Ok(sessions)
}

fn claude_session_from_transcript(path: &Path, id: String) -> Result<Option<Session>> {
    let mut title = None;
    let mut cwd = None;
    let mut created_at = None;
    let mut updated_at = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(DateTime::<Utc>::from);
    let mut message_count = 0usize;
    let mut latest = Vec::new();

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(|line| line.ok()).take(300) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(ts) = string_field(&value, "timestamp").and_then(|t| parse_time(Some(&t))) {
            updated_at = Some(ts);
            created_at.get_or_insert(ts);
        }
        cwd = string_field(&value, "cwd").map(PathBuf::from).or(cwd);
        let message_type = value.get("type").and_then(Value::as_str);
        if matches!(message_type, Some("user") | Some("assistant")) {
            if let Some(message) = value.get("message") {
                let text = text_from_value(message.get("content").unwrap_or(message));
                if !text.is_empty() {
                    message_count += 1;
                    title.get_or_insert_with(|| short_title(&text));
                    if message_type == Some("user") {
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
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut snippets = Vec::new();
    for line in reader.lines().map_while(|line| line.ok()) {
        let text = searchable_line_text(&line);
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
    let walker = if recursive {
        WalkDir::new(root)
    } else {
        WalkDir::new(root).max_depth(1)
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
