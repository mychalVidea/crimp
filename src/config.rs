use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Nepodařilo se určit konfigurační adresář (~/.config)")]
    NoConfigDir,
    #[error("Chyba I/O při práci s konfigurací '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Chyba parsování TOML konfigurace '{path}': {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NodeMatcher {
    pub node_name: Option<String>,
    pub app_name: Option<String>,
    pub media_class: Option<String>,
    pub port_name: Option<String>,
}

impl NodeMatcher {
    pub fn is_empty(&self) -> bool {
        self.node_name.is_none()
            && self.app_name.is_none()
            && self.media_class.is_none()
            && self.port_name.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenyRule {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub source: NodeMatcher,
    #[serde(default)]
    pub target: NodeMatcher,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteRule {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub source: NodeMatcher,
    #[serde(default)]
    pub target: NodeMatcher,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub deny: Vec<DenyRule>,
    pub route: Vec<RouteRule>,
}

fn default_enabled() -> bool {
    true
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.join("crimp").join("rules.toml"))
}

pub fn default_rules_toml() -> &'static str {
    r#"# Konfigurace Crimp (~/.config/crimp/rules.toml)
# Vzory podporují wildcard '*' (case-insensitive).

# Blokované automatické linky (Crimp je okamžitě rozpojí přes destroy_global):
# [[deny]]
# name = "block-example"
# enabled = true
# [deny.source]
# app_name = "OBS*"
# [deny.target]
# node_name = "*virtual-mic*"

# Připravená routovací pravidla (vynucení cílového sinku pro aplikaci):
# [[route]]
# name = "route-example"
# enabled = false
# [route.source]
# app_name = "Discord*"
# [route.target]
# node_name = "crimp-comms-sink"
"#
}

pub fn load_or_create_default(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(path, default_rules_toml()).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str::<Config>(&raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_template_as_empty_rules() {
        let cfg: Config = toml::from_str(default_rules_toml()).expect("valid default TOML");
        assert!(cfg.deny.is_empty());
        assert!(cfg.route.is_empty());
    }

    #[test]
    fn parses_deny_and_route_rules() {
        let raw = r#"
            [[deny]]
            name = "block-obs"
            [deny.source]
            app_name = "OBS*"
            [deny.target]
            node_name = "alsa_input.*"

            [[route]]
            name = "route-discord"
            enabled = false
            [route.source]
            app_name = "Discord*"
            [route.target]
            node_name = "comms-sink"
        "#;
        let cfg: Config = toml::from_str(raw).expect("parse rules");
        assert_eq!(cfg.deny.len(), 1);
        assert!(cfg.deny[0].enabled);
        assert_eq!(cfg.deny[0].source.app_name.as_deref(), Some("OBS*"));
        assert_eq!(cfg.route.len(), 1);
        assert!(!cfg.route[0].enabled);
        assert_eq!(cfg.route[0].target.node_name.as_deref(), Some("comms-sink"));
    }
}
