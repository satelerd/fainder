use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::model::ProviderKind;

#[derive(Clone, Debug)]
pub struct Config {
    pub home: PathBuf,
    pub paths: HashMap<ProviderKind, PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    paths: Option<HashMap<String, PathBuf>>,
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

        let config_path = dirs::config_dir()
            .unwrap_or_else(|| home.join(".config"))
            .join("fainder/config.toml");
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
        }

        Ok(Self { home, paths })
    }

    pub fn path(&self, provider: ProviderKind) -> PathBuf {
        self.paths
            .get(&provider)
            .cloned()
            .unwrap_or_else(|| self.home.clone())
    }
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
