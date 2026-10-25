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
    /// Additional roots read after the primary path, e.g. a second Codex home
    /// such as `~/.codex-orq`.
    pub extra_paths: HashMap<ProviderKind, Vec<PathBuf>>,
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
    paths: Option<HashMap<String, PathOrPaths>>,
    team: Option<TeamConfig>,
}

/// `codex = "~/.codex"` or `codex = ["~/.codex", "~/.codex-orq"]`; the first
/// entry replaces the default path and the rest are read as extra roots.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PathOrPaths {
    One(PathBuf),
    Many(Vec<PathBuf>),
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

        let mut extra_paths: HashMap<ProviderKind, Vec<PathBuf>> = HashMap::new();
        extra_paths.insert(ProviderKind::Codex, discovered_codex_homes(&home));

        let mut team = TeamConfig::default();
        let config_path = config_path(&home);
        if config_path.exists() {
            let raw = fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read {}", config_path.display()))?;
            let parsed: FileConfig = toml::from_str(&raw)
                .with_context(|| format!("failed to parse {}", config_path.display()))?;
            if let Some(overrides) = parsed.paths {
                for (key, value) in overrides {
                    let Ok(provider) = key.parse::<ProviderKind>() else {
                        continue;
                    };
                    let mut list = match value {
                        PathOrPaths::One(path) => vec![path],
                        PathOrPaths::Many(list) => list,
                    }
                    .into_iter()
                    .map(|path| expand_tilde(path, &home));
                    if let Some(primary) = list.next() {
                        paths.insert(provider, primary);
                    }
                    let extras = extra_paths.entry(provider).or_default();
                    extras.extend(list);
                }
            }
            if let Some(parsed_team) = parsed.team {
                team = parsed_team;
            }
        }

        // Drop extras that repeat the primary path or each other.
        for (provider, extras) in extra_paths.iter_mut() {
            let primary = paths.get(provider).cloned();
            let mut seen = Vec::new();
            extras.retain(|path| {
                let keep = Some(path) != primary.as_ref() && !seen.contains(path);
                seen.push(path.clone());
                keep
            });
        }

        Ok(Self {
            home,
            paths,
            extra_paths,
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

    /// The primary path followed by any extra roots for this provider.
    pub fn roots(&self, provider: ProviderKind) -> Vec<PathBuf> {
        let mut roots = vec![self.path(provider)];
        if let Some(extras) = self.extra_paths.get(&provider) {
            roots.extend(extras.iter().cloned());
        }
        roots
    }
}

/// Codex homes besides `~/.codex`: `$CODEX_HOME` and sibling `~/.codex-*`
/// directories that hold a `sessions/` folder (a second Codex profile, such as
/// one an orchestrator launches with its own `CODEX_HOME`).
fn discovered_codex_homes(home: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(env_home) = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
    {
        homes.push(env_home);
    }
    let mut siblings = fs::read_dir(home)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".codex-"))
                && path.join("sessions").is_dir()
        })
        .collect::<Vec<_>>();
    siblings.sort();
    homes.extend(siblings);
    homes
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn discovers_sibling_codex_homes_with_sessions() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("fainder-config-test-{nanos}"));
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        fs::create_dir_all(home.join(".codex-orq/sessions")).unwrap();
        fs::create_dir_all(home.join(".codex-empty")).unwrap();
        fs::create_dir_all(home.join(".codexbar/sessions")).unwrap();

        let found = discovered_codex_homes(&home)
            .into_iter()
            .filter(|path| path.starts_with(&home))
            .collect::<Vec<_>>();
        let _ = fs::remove_dir_all(&home);
        assert_eq!(found, vec![home.join(".codex-orq")]);
    }
}
