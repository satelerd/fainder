use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Codex,
    Claude,
    Opencode,
    Hermes,
    Cursor,
    Copilot,
}

impl ProviderKind {
    /// Number of provider variants (keep in sync with `all`).
    pub const COUNT: usize = 6;

    pub fn all() -> Vec<Self> {
        vec![
            Self::Codex,
            Self::Claude,
            Self::Opencode,
            Self::Hermes,
            Self::Cursor,
            Self::Copilot,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Hermes => "hermes",
            Self::Cursor => "cursor",
            Self::Copilot => "copilot",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "codex" => Ok(Self::Codex),
            "claude" | "claude-code" | "cloud-code" => Ok(Self::Claude),
            "opencode" | "open-code" => Ok(Self::Opencode),
            "hermes" => Ok(Self::Hermes),
            "cursor" => Ok(Self::Cursor),
            "copilot" | "github-copilot" | "vscode-copilot" => Ok(Self::Copilot),
            _ => Err(anyhow!("unknown provider: {value}")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub provider: ProviderKind,
    pub id: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub message_count: Option<usize>,
    pub source_path: Option<PathBuf>,
    pub transcript_path: Option<PathBuf>,
    pub resume_command: String,
    pub latest_messages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchResult {
    pub provider: ProviderKind,
    pub id: String,
    pub title: String,
    pub cwd: Option<PathBuf>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub message_count: Option<usize>,
    pub resume_command: String,
    pub score: i64,
    pub matched_in: String,
    pub snippets: Vec<String>,
    pub latest_messages: Vec<String>,
}

/// How the query text is interpreted when matching conversations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchMode {
    /// Match the whole query as one literal substring (case-insensitive).
    Phrase,
    /// Split the query into words and require all of them (any order).
    Words,
    /// Treat the query as a case-insensitive regular expression.
    Regex,
}

impl SearchMode {
    /// Short label shown in the TUI header and CLI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Phrase => "phrase",
            Self::Words => "words",
            Self::Regex => "regex",
        }
    }

    /// Cycle to the next mode for the TUI toggle.
    pub fn next(self) -> Self {
        match self {
            Self::Phrase => Self::Words,
            Self::Words => Self::Regex,
            Self::Regex => Self::Phrase,
        }
    }

    /// Human sentence describing how `query` is being matched right now.
    pub fn describe(self, query: &str) -> String {
        let query = query.trim();
        match self {
            Self::Phrase => format!("Matching the exact phrase \"{query}\""),
            Self::Words => {
                let words: Vec<&str> = query.split_whitespace().collect();
                if words.len() <= 1 {
                    format!("Matching conversations that contain \"{query}\"")
                } else {
                    format!(
                        "Matching conversations that contain {}",
                        words.join(" AND ")
                    )
                }
            }
            Self::Regex => format!("Matching the regular expression /{query}/i"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub providers: Vec<ProviderKind>,
    pub mode: SearchMode,
    pub limit: usize,
    pub full_text: bool,
}

#[derive(Clone, Debug)]
pub struct ProviderReport {
    pub provider: ProviderKind,
    pub path: PathBuf,
    pub exists: bool,
    pub sessions: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptRole {
    User,
    Agent,
    Tool,
    System,
    Unknown,
}

impl TranscriptRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::System => "system",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptTurn {
    pub turn: usize,
    pub role: TranscriptRole,
    pub timestamp: Option<DateTime<Utc>>,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_result: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Transcript {
    pub session: Session,
    pub turns: Vec<TranscriptTurn>,
}
