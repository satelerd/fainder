use std::collections::HashMap;

use anyhow::Result;
use regex::{Regex, RegexBuilder};

use crate::config::Config;
use crate::model::{ProviderKind, SearchMode, SearchOptions, SearchResult, Session};
use crate::providers;

pub fn recent(
    config: &Config,
    providers: &[ProviderKind],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let providers_to_list = if providers.is_empty() {
        ProviderKind::all()
    } else {
        providers.to_vec()
    };
    let mut results = Vec::new();

    for provider in providers_to_list {
        let sessions = match providers::recent_sessions(config, provider, limit) {
            Ok(sessions) => sessions,
            Err(_) => continue,
        };
        for session in sessions {
            results.push(session_result(session, 0, "recent".to_string(), Vec::new()));
        }
    }

    sort_results(&mut results);
    results.truncate(limit.max(1));
    Ok(results)
}

pub fn search(config: &Config, options: &SearchOptions) -> Result<Vec<SearchResult>> {
    let query = options.query.trim();
    if query.is_empty() {
        return recent(config, &options.providers, options.limit);
    }

    let providers_to_search = if options.providers.is_empty() {
        ProviderKind::all()
    } else {
        options.providers.clone()
    };

    let matcher = Matcher::new(query, options.mode)?;
    let mut by_key: HashMap<(ProviderKind, String), SearchResult> = HashMap::new();

    for provider in &providers_to_search {
        let sessions = match providers::sessions(config, *provider) {
            Ok(sessions) => sessions,
            Err(_) => continue,
        };
        for session in sessions {
            let mut matched_in = Vec::new();
            let mut snippets = Vec::new();
            let mut score = 0;

            if matcher.is_match(&session.title) {
                score += 1000;
                matched_in.push("title");
                snippets.push(highlightless_snippet(&session.title, query));
            }

            if let Some(cwd) = &session.cwd {
                let cwd_text = cwd.display().to_string();
                if matcher.is_match(&cwd_text) {
                    score += 600;
                    matched_in.push("path");
                    snippets.push(highlightless_snippet(&cwd_text, query));
                }
            }

            for message in &session.latest_messages {
                if matcher.is_match(message) {
                    score += 250;
                    matched_in.push("latest");
                    snippets.push(highlightless_snippet(message, query));
                    break;
                }
            }

            if score > 0 {
                upsert_result(&mut by_key, session, score, matched_in.join(","), snippets);
            }
        }
    }

    if options.full_text && by_key.len() < options.limit {
        for provider in providers_to_search {
            if by_key.len() >= options.limit {
                break;
            }
            let Ok(content_sessions) =
                providers::content_sessions(config, provider, &matcher, query, options.limit * 8)
            else {
                continue;
            };
            for (session, snippets) in content_sessions {
                upsert_result(&mut by_key, session, 200, "content".to_string(), snippets);
            }
        }
    }

    let mut results: Vec<_> = by_key.into_values().collect();
    sort_results(&mut results);
    results.truncate(options.limit.max(1));
    Ok(results)
}

fn sort_results(results: &mut [SearchResult]) {
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.provider.label().cmp(b.provider.label()))
            .then_with(|| a.title.cmp(&b.title))
    });
}

fn upsert_result(
    by_key: &mut HashMap<(ProviderKind, String), SearchResult>,
    session: Session,
    score: i64,
    matched_in: String,
    snippets: Vec<String>,
) {
    let key = (session.provider, session.id.clone());
    let result = session_result(session, score, matched_in, snippets);

    match by_key.get_mut(&key) {
        Some(existing) => {
            existing.score = existing.score.max(result.score);
            if existing.cwd.is_none() && result.cwd.is_some() {
                existing.cwd = result.cwd;
                existing.resume_command = result.resume_command;
            }
            if existing.created_at.is_none() && result.created_at.is_some() {
                existing.created_at = result.created_at;
            }
            if existing.updated_at.is_none() && result.updated_at.is_some() {
                existing.updated_at = result.updated_at;
            }
            if existing.message_count.is_none() && result.message_count.is_some() {
                existing.message_count = result.message_count;
            }
            for part in result.matched_in.split(',') {
                if !part.is_empty() && !existing.matched_in.split(',').any(|p| p == part) {
                    if !existing.matched_in.is_empty() {
                        existing.matched_in.push(',');
                    }
                    existing.matched_in.push_str(part);
                }
            }
            existing.snippets.extend(result.snippets);
            existing.snippets = bounded(std::mem::take(&mut existing.snippets), 4);
            existing.latest_messages.extend(result.latest_messages);
            existing.latest_messages = bounded(std::mem::take(&mut existing.latest_messages), 5);
        }
        None => {
            by_key.insert(key, result);
        }
    }
}

fn session_result(
    session: Session,
    score: i64,
    matched_in: String,
    snippets: Vec<String>,
) -> SearchResult {
    SearchResult {
        provider: session.provider,
        id: session.id,
        title: session.title,
        cwd: session.cwd,
        created_at: session.created_at,
        updated_at: session.updated_at,
        message_count: session.message_count,
        resume_command: session.resume_command,
        score,
        matched_in,
        snippets: bounded(snippets, 4),
        latest_messages: bounded(session.latest_messages, 5),
    }
}

fn bounded<T>(items: Vec<T>, limit: usize) -> Vec<T> {
    items.into_iter().take(limit).collect()
}

pub enum Matcher {
    /// Whole query as one case-insensitive substring.
    Phrase(String),
    /// All words must be present (case-insensitive, any order).
    Words(Vec<String>),
    /// Case-insensitive regular expression.
    Regex(Regex),
}

impl Matcher {
    pub fn new(query: &str, mode: SearchMode) -> Result<Self> {
        Ok(match mode {
            SearchMode::Regex => {
                Self::Regex(RegexBuilder::new(query).case_insensitive(true).build()?)
            }
            SearchMode::Phrase => Self::Phrase(query.trim().to_lowercase()),
            SearchMode::Words => Self::Words(
                query
                    .split_whitespace()
                    .map(|part| part.to_lowercase())
                    .collect(),
            ),
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(text),
            Self::Phrase(needle) => text.to_lowercase().contains(needle.as_str()),
            Self::Words(terms) => {
                let text = text.to_lowercase();
                terms.iter().all(|term| text.contains(term))
            }
        }
    }
}

pub fn highlightless_snippet(text: &str, query: &str) -> String {
    let text = text.replace('\n', " ");
    let lower = text.to_lowercase();
    let needle = query
        .split_whitespace()
        .next()
        .unwrap_or(query)
        .to_lowercase();
    let byte_index = lower.find(&needle).unwrap_or(0);
    let char_index = text[..byte_index].chars().count();
    let start = char_index.saturating_sub(80);
    let end = char_index + needle.chars().count() + 180;
    let chars = text.chars().collect::<Vec<_>>();
    let end = end.min(chars.len());
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < chars.len() { "..." } else { "" };
    let snippet = chars[start..end].iter().collect::<String>();
    format!("{prefix}{}{suffix}", snippet.trim())
}

#[cfg(test)]
mod tests {
    use super::{Matcher, highlightless_snippet};
    use crate::model::SearchMode;

    #[test]
    fn words_mode_requires_all_terms_case_insensitive() {
        let matcher = Matcher::new("smartup crítico", SearchMode::Words).unwrap();
        assert!(matcher.is_match("SmartUp agente crítico"));
        assert!(!matcher.is_match("SmartUp agente"));
    }

    #[test]
    fn phrase_mode_matches_literal_substring_only() {
        let matcher = Matcher::new("SmartUp agente", SearchMode::Phrase).unwrap();
        assert!(matcher.is_match("hablemos del SmartUp agente nuevo"));
        // Words present but not adjacent in order must not match a phrase.
        assert!(!matcher.is_match("agente de SmartUp"));
    }

    #[test]
    fn snippet_handles_unicode_boundaries() {
        let snippet = highlightless_snippet(
            "Buenas, necesito revisar SmartUp. En promedio, ¿cuánto demora?",
            "smartup",
        );
        assert!(snippet.contains("SmartUp"));
        assert!(snippet.contains("¿cuánto"));
    }
}
