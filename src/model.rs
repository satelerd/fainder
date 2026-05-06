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
    pub updated_at: Option<DateTime<Utc>>,
    pub resume_command: String,
    pub score: i64,
    pub matched_in: String,
    pub snippets: Vec<String>,
    pub latest_messages: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub providers: Vec<ProviderKind>,
    pub regex: bool,
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
