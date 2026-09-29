use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::ProviderKind;

#[derive(Clone, Debug)]
pub struct Config {
    pub home: PathBuf,
    pub paths: HashMap<ProviderKind, PathBuf>,
    pub team: TeamConfig,
    pub config_path: PathBuf,
}

/// `[team]` section: where `fainder team` sends queries. The key is read from
/// an environment variable, never from this file, so the config can be shared
/// without leaking the credential.
#[derive(Clone, Debug, Deserialize)]
pub struct TeamConfig {
    pub url: Option<String>,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            url: None,
            api_key_env: default_api_key_env(),
        }
    }
}

fn default_api_key_env() -> String {
    "FAINDER_TEAM_KEY".to_string()
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    paths: Option<HashMap<String, PathBuf>>,
    team: Option<TeamConfig>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().context("could not detect home directory")?;
        let mut paths = HashMap::new();
        paths.insert(ProviderKind::Codex, home.join(".codex"));
        paths.insert(ProviderKind::Claude, home.join(".claude"));
        paths.insert(
            ProviderKind::Opencode,
            home.join(".local/share/opencode/opencode.db"),
        );
        paths.insert(ProviderKind::Hermes, home.join(".hermes/sessions"));
        // ~/Library/Application Support on macOS; ~/.config and ~/.local/share on Linux.
        let app_config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        let app_data = dirs::data_dir().unwrap_or_else(|| home.join(".local/share"));
        paths.insert(
            ProviderKind::Cursor,
            app_config.join("Cursor/User/workspaceStorage"),
        );
        paths.insert(
            ProviderKind::Copilot,
            app_config.join("Code/User/workspaceStorage"),
        );
        paths.insert(ProviderKind::Kiro, app_data.join("kiro-cli/data.sqlite3"));

        let mut team = TeamConfig::default();
        let config_path = config_path(&home);
        if config_path.exists() {
            let raw = fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            let parsed: FileConfig = toml::from_str(&raw)
                .with_context(|| format!("failed to parse {}", config_path.display()))?;
            if let Some(overrides) = parsed.paths {
                for (key, path) in overrides {
                    if let Ok(provider) = key.parse::<ProviderKind>() {
                        paths.insert(provider, expand_tilde(path, &home));
                    }
                }
            }
            if let Some(parsed_team) = parsed.team {
                team = parsed_team;
            }
        }

        Ok(Self {
            home,
            paths,
            team,
            config_path,
        })
    }

    pub fn path(&self, provider: ProviderKind) -> PathBuf {
        self.paths
            .get(&provider)
            .cloned()
            .unwrap_or_else(|| self.home.clone())
    }
}

/// `$XDG_CONFIG_HOME/fainder/config.toml`, else `~/.config/fainder/config.toml`,
/// which is what the docs promise on every OS. On macOS `dirs::config_dir()` is
/// `~/Library/Application Support`, so that location is only a fallback for
/// configs that already live there.
pub fn config_path(home: &Path) -> PathBuf {
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("fainder/config.toml");
    if xdg.exists() {
        return xdg;
    }
    dirs::config_dir()
        .map(|dir| dir.join("fainder/config.toml"))
        .filter(|legacy| legacy.exists())
        .unwrap_or(xdg)
}

fn expand_tilde(path: PathBuf, home: &PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        home.clone()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        path
    }
}
