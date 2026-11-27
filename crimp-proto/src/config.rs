use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Chyby při načítání, validaci nebo ukládání konfiguračního souboru.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Nepodařilo se určit konfigurační adresář (~/.config)")]
    NoConfigDir,

    #[error("I/O chyba při práci s konfigurací ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Chyba při parsování TOML konfigurace ({path}): {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Chyba serializace TOML konfigurace: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("Neplatná konfigurace ({rule}): {reason}")]
    Validation { rule: String, reason: String },
}

/// Výběr PipeWire uzlu a volitelně konkrétního portu.
/// Všechny zadané položky musí vyhovovat (logické AND); podporuje case-insensitive glob (`*`, `?`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NodeMatcher {
    pub node_name: Option<String>,
    pub application_name: Option<String>,
    pub media_class: Option<String>,
    pub port_name: Option<String>,
}

impl NodeMatcher {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.node_name.is_none()
            && self.application_name.is_none()
            && self.media_class.is_none()
            && self.port_name.is_none()
    }
}

/// Pravidlo pro tvrdé blokování (okamžité rozpojení) nechtěných linků.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenyRule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: NodeMatcher,
    pub target: NodeMatcher,
}

/// Pravidlo pro aktivní vynucené routování aplikace/zdroje na cílový uzel/sink.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteRule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: NodeMatcher,
    pub target: NodeMatcher,
    /// Pokud je `true` (výchozí), zničí konkurenční linky ze stejného zdroje na jiné cíle,
    /// které si tam automaticky vytvořil WirePlumber.
    #[serde(default = "default_true")]
    pub exclusive: bool,
}

/// Deklarativní virtuální audio sink (null sink), který Crimp vytvoří a udrží v PipeWire grafu.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualSinkRule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_virtual_media_class")]
    pub media_class: String,
    #[serde(default = "default_stereo_channels")]
    pub channels: u32,
    #[serde(default = "default_stereo_positions")]
    pub positions: Vec<String>,
}

/// Výchozí nastavení PipeWire hodin (`metadata` -> `"settings"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DspConfig {
    pub default_quantum: Option<u32>,
    pub default_rate: Option<u32>,
}

/// Rozšířená globální nastavení PipeWire (`metadata` `"settings"` a `"default"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PwSettingsConfig {
    pub min_quantum: Option<u32>,
    pub max_quantum: Option<u32>,
    pub quantum_limit: Option<u32>,
    pub quantum_floor: Option<u32>,
    pub allowed_rates: Option<Vec<u32>>,
    pub power_of_two_quantum: Option<bool>,
    pub log_level: Option<u8>,
    pub default_audio_sink: Option<String>,
    pub default_audio_source: Option<String>,
}

/// Dynamický DSP profil: pokud v grafu existuje odpovídající uzel (např. DAW nebo hra),
/// přepne PipeWire quantum / sample rate v reálném čase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DspProfileRule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: NodeMatcher,
    pub quantum: Option<u32>,
    pub rate: Option<u32>,
    #[serde(default)]
    pub priority: i32,
}

/// Uživatelské přejmenování procesu nebo audio zařízení pouze pro přehledné zobrazení.
/// Veškerá PipeWire pravidla a routy nadále používají originální `node.name` / `application.name`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasRule {
    pub matcher: NodeMatcher,
    pub display_name: String,
}

/// Konfigurace lokálního Unix Domain Socketu pro CLI a GUI klienty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IpcConfig {
    pub enabled: bool,
    pub socket_path: Option<String>,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket_path: None,
        }
    }
}

/// Kořenová konfigurace Crimpu (`~/.config/crimp/rules.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub ipc: IpcConfig,
    pub dsp: DspConfig,
    pub pw_settings: PwSettingsConfig,
    pub virtual_sink: Vec<VirtualSinkRule>,
    pub deny: Vec<DenyRule>,
    pub route: Vec<RouteRule>,
    pub dsp_profile: Vec<DspProfileRule>,
    pub alias: Vec<AliasRule>,
}

impl Config {
    /// Zkontroluje logickou konzistenci pravidel před jejich nasazením v PipeWire vlákně.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for sink in &self.virtual_sink {
            if sink.name.trim().is_empty() {
                return Err(ConfigError::Validation {
                    rule: "virtual_sink".into(),
                    reason: "Název virtuálního sinku (`name`) nesmí být prázdný".into(),
                });
            }
            if sink.channels == 0 || sink.channels > 32 {
                return Err(ConfigError::Validation {
                    rule: sink.name.clone(),
                    reason: format!("Počet kanálů ({}) musí být v rozsahu 1..=32", sink.channels),
                });
            }
            if !sink.positions.is_empty() && sink.positions.len() != sink.channels as usize {
                return Err(ConfigError::Validation {
                    rule: sink.name.clone(),
                    reason: format!(
                        "Počet položek v `positions` ({}) neodpovídá `channels` ({})",
                        sink.positions.len(),
                        sink.channels
                    ),
                });
            }
        }

        validate_opt_quantum("dsp.default_quantum", self.dsp.default_quantum)?;
        validate_opt_rate("dsp.default_rate", self.dsp.default_rate)?;

        validate_opt_quantum("pw_settings.min_quantum", self.pw_settings.min_quantum)?;
        validate_opt_quantum("pw_settings.max_quantum", self.pw_settings.max_quantum)?;
        validate_opt_quantum("pw_settings.quantum_limit", self.pw_settings.quantum_limit)?;
        validate_opt_quantum("pw_settings.quantum_floor", self.pw_settings.quantum_floor)?;

        if let (Some(min_q), Some(max_q)) =
            (self.pw_settings.min_quantum, self.pw_settings.max_quantum)
            && min_q > max_q
        {
            return Err(ConfigError::Validation {
                rule: "pw_settings".into(),
                reason: format!("min_quantum ({min_q}) nesmí být větší než max_quantum ({max_q})"),
            });
        }

        if let Some(rates) = &self.pw_settings.allowed_rates {
            if rates.is_empty() {
                return Err(ConfigError::Validation {
                    rule: "pw_settings.allowed_rates".into(),
                    reason: "Seznam allowed_rates nesmí být prázdný".into(),
                });
            }
            for &rate in rates {
                validate_opt_rate("pw_settings.allowed_rates", Some(rate))?;
            }
        }

        if let Some(lvl) = self.pw_settings.log_level
            && lvl > 6
        {
            return Err(ConfigError::Validation {
                rule: "pw_settings.log_level".into(),
                reason: format!("PipeWire log_level ({lvl}) musí být v rozsahu 0..=6"),
            });
        }

        for profile in &self.dsp_profile {
            if profile.source.is_empty() {
                return Err(ConfigError::Validation {
                    rule: profile.name.clone(),
                    reason: "DSP profil musí specifikovat alespoň jedno pole v `source`".into(),
                });
            }
            validate_opt_quantum(&profile.name, profile.quantum)?;
            validate_opt_rate(&profile.name, profile.rate)?;
        }

        for rule in &self.deny {
            if rule.source.is_empty() || rule.target.is_empty() {
                return Err(ConfigError::Validation {
                    rule: rule.name.clone(),
                    reason: "Deny pravidlo musí obsahovat neprázdný `source` i `target`".into(),
                });
            }
        }

        for rule in &self.route {
            if rule.source.is_empty() || rule.target.is_empty() {
                return Err(ConfigError::Validation {
                    rule: rule.name.clone(),
                    reason: "Route pravidlo musí obsahovat neprázdný `source` i `target`".into(),
                });
            }
        }

        for a in &self.alias {
            if a.matcher.is_empty() || a.display_name.trim().is_empty() {
                return Err(ConfigError::Validation {
                    rule: "alias".into(),
                    reason: "Alias musí obsahovat neprázdný `matcher` a `display_name`".into(),
                });
            }
        }

        Ok(())
    }

    /// Promítne ruční změnu PipeWire metadata/DSP do konfigurace tak, aby přežila restart PC.
    /// Vrací `Ok(true)`, pokud se změnila alespoň jedna perzistentní položka konfigurace.
    pub fn apply_metadata_override(
        &mut self,
        key: &str,
        value: Option<&str>,
    ) -> Result<bool, String> {
        let clean = value.map(str::trim).filter(|s| !s.is_empty());
        let mut next = self.clone();

        match key.trim() {
            "clock.force-quantum" => {
                next.dsp.default_quantum = match clean {
                    None | Some("0") => None,
                    Some(v) => Some(
                        v.parse::<u32>()
                            .map_err(|_| format!("Neplatná hodnota clock.force-quantum '{v}'"))?,
                    ),
                };
            }
            "clock.force-rate" => {
                next.dsp.default_rate = match clean {
                    None | Some("0") => None,
                    Some(v) => Some(
                        v.parse::<u32>()
                            .map_err(|_| format!("Neplatná hodnota clock.force-rate '{v}'"))?,
                    ),
                };
            }
            "clock.min-quantum" => {
                next.pw_settings.min_quantum = parse_opt_pw_u32("clock.min-quantum", clean)?;
            }
            "clock.max-quantum" => {
                next.pw_settings.max_quantum = parse_opt_pw_u32("clock.max-quantum", clean)?;
            }
            "clock.quantum-limit" => {
                next.pw_settings.quantum_limit = parse_opt_pw_u32("clock.quantum-limit", clean)?;
            }
            "clock.quantum-floor" => {
                next.pw_settings.quantum_floor = parse_opt_pw_u32("clock.quantum-floor", clean)?;
            }
            "clock.allowed-rates" => {
                next.pw_settings.allowed_rates = match clean {
                    None => None,
                    Some(v) => Some(parse_allowed_rates_list(v)?),
                };
            }
            "clock.power-of-two-quantum" => {
                next.pw_settings.power_of_two_quantum = match clean {
                    None => None,
                    Some("true" | "1") => Some(true),
                    Some("false" | "0") => Some(false),
                    Some(other) => {
                        return Err(format!(
                            "Neplatná booleovská hodnota pro '{key}': '{other}' (očekáváno true/false)"
                        ));
                    }
                };
            }
            "log.level" => {
                next.pw_settings.log_level = match clean {
                    None => None,
                    Some(v) => Some(
                        v.parse::<u8>()
                            .map_err(|_| format!("Neplatná hodnota log.level '{v}'"))?,
                    ),
                };
            }
            "default.audio.sink" | "default.configured.audio.sink" => {
                next.pw_settings.default_audio_sink =
                    crate::graph::PwRuntimeState::extract_device_name(clean);
            }
            "default.audio.source" | "default.configured.audio.source" => {
                next.pw_settings.default_audio_source =
                    crate::graph::PwRuntimeState::extract_device_name(clean);
            }
            _ => return Ok(false),
        }

        next.validate().map_err(|e| e.to_string())?;
        if &next == self {
            return Ok(false);
        }
        *self = next;
        Ok(true)
    }
}

/// Zparsuje seznam vzorkovacích frekvencí z formátu PipeWire (`[ 44100 48000 96000 ]` nebo `44100, 48000`).
pub fn parse_allowed_rates_list(raw: &str) -> Result<Vec<u32>, String> {
    let inner = raw.trim().trim_start_matches('[').trim_end_matches(']');
    let mut rates = Vec::new();
    for token in inner.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let t = token.trim();
        if t.is_empty() {
            continue;
        }
        let val = t
            .parse::<u32>()
            .map_err(|_| format!("Neplatná vzorkovací frekvence '{t}' v allowed-rates"))?;
        rates.push(val);
    }
    if rates.is_empty() {
        return Err("Seznam clock.allowed-rates nesmí být prázdný".to_string());
    }
    Ok(rates)
}

fn parse_opt_pw_u32(key: &str, clean: Option<&str>) -> Result<Option<u32>, String> {
    match clean {
        None => Ok(None),
        Some(v) => v
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("Neplatné číslo pro {key}: '{v}'")),
    }
}

fn validate_opt_quantum(rule: &str, quantum: Option<u32>) -> Result<(), ConfigError> {
    if let Some(q) = quantum
        && q == 0
    {
        return Err(ConfigError::Validation {
            rule: rule.to_string(),
            reason: "Quantum musí být kladné číslo (např. 64, 128, 1024)".into(),
        });
    }
    Ok(())
}

fn validate_opt_rate(rule: &str, rate: Option<u32>) -> Result<(), ConfigError> {
    if let Some(r) = rate
        && r < 8_000
    {
        return Err(ConfigError::Validation {
            rule: rule.to_string(),
            reason: format!("Sample rate {r} Hz je příliš nízký (minimum 8000 Hz)"),
        });
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

fn default_virtual_media_class() -> String {
    "Audio/Sink".to_string()
}

const fn default_stereo_channels() -> u32 {
    2
}

fn default_stereo_positions() -> Vec<String> {
    vec!["FL".to_string(), "FR".to_string()]
}

/// Výchozí cesta k pravidlům: `~/.config/crimp/rules.toml`.
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.join("crimp").join("rules.toml"))
}

/// Zformátovaná ukázková konfigurace generovaná při prvním spuštění.
#[must_use]
pub fn default_rules_toml() -> &'static str {
    r#"# Crimp – PipeWire routing, virtual-sink, settings & blocklist pravidla
# Dokumentace a příklady: https://github.com/mychalVidea/crimp
#
# Změny v tomto souboru se aplikují automaticky za běhu (hot-reload),
# případně příkazem `crimp reload`.

[ipc]
enabled = true
# socket_path = "/run/user/1000/crimp.sock"

[dsp]
# Výchozí PipeWire quantum / sample rate při absenci aktivního [[dsp_profile]]
# default_quantum = 1024
# default_rate = 48000

[pw_settings]
# Globální metadata nastavení PipeWire (`settings` & `default`)
# min_quantum = 64
# max_quantum = 2048
# allowed_rates = [44100, 48000, 96000]
# power_of_two_quantum = true

# --- Ukázka: Virtuální audio sběrnice (Null Sink) ---
# [[virtual_sink]]
# name = "crimp_stream_mix"
# description = "Crimp Stream Mix"
# media_class = "Audio/Sink"
# channels = 2
# positions = ["FL", "FR"]

# --- Ukázka: Tvrdý blocklist nechtěných auto-linků WirePlumberu ---
# [[deny]]
# name = "Zákaz monitoringu HDMI výstupu do Discordu"
# enabled = true
# source = { node_name = "alsa_output.*hdmi*.monitor" }
# target = { application_name = "WEBRTC VoiceEngine" }

# --- Ukázka: Vynucené routování aplikace na konkrétní sink ---
# [[route]]
# name = "Firefox do Crimp virtuální sběrnice"
# enabled = true
# exclusive = true
# source = { application_name = "Firefox" }
# target = { node_name = "crimp_stream_mix" }

# --- Ukázka: Dynamické snížení latence pro DAW ---
# [[dsp_profile]]
# name = "Reaper low-latency"
# enabled = true
# priority = 100
# source = { application_name = "REAPER" }
# quantum = 128
# rate = 48000
"#
}

/// Načte konfiguraci z disku; pokud soubor neexistuje, vytvoří ho se šablonou.
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

    load_existing(path)
}

/// Načte a zvaliduje existující konfigurační soubor (vhodné pro hot-reload).
pub fn load_existing(path: &Path) -> Result<Config, ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let cfg: Config = toml::from_str(&raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    cfg.validate()?;
    Ok(cfg)
}

/// Zvaliduje a atomicky uloží novou konfiguraci do souboru (pro úpravy z GUI).
pub fn save_config(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    cfg.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let body = toml::to_string_pretty(cfg)?;
    let header = "# Generováno a spravováno pomocí Crimp (`rules.toml`)\n\n";
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, format!("{header}{body}")).map_err(|source| ConfigError::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses_and_validates() {
        let cfg: Config = toml::from_str(default_rules_toml()).expect("valid default TOML");
        cfg.validate().expect("default config validates cleanly");
        assert!(cfg.ipc.enabled);
        assert!(cfg.virtual_sink.is_empty());
        assert!(cfg.deny.is_empty());
        assert!(cfg.route.is_empty());
        assert!(cfg.dsp_profile.is_empty());
    }

    #[test]
    fn parses_full_extended_config_with_pw_settings() {
        let sample = r#"
            [ipc]
            enabled = true
            socket_path = "/tmp/crimp-test.sock"

            [dsp]
            default_quantum = 1024
            default_rate = 48000

            [pw_settings]
            min_quantum = 64
            max_quantum = 2048
            allowed_rates = [44100, 48000, 96000]
            power_of_two_quantum = true
            log_level = 2
            default_audio_sink = "crimp_game"

            [[virtual_sink]]
            name = "crimp_game"
            description = "Game Bus"
            channels = 2
            positions = ["FL", "FR"]

            [[deny]]
            name = "Block mic monitor"
            source = { node_name = "alsa_input.*" }
            target = { application_name = "Firefox" }

            [[route]]
            name = "Pin Discord to headphones"
            exclusive = false
            source = { application_name = "WEBRTC VoiceEngine" }
            target = { node_name = "alsa_output.usb-Headset.*" }

            [[dsp_profile]]
            name = "Studio"
            priority = 10
            source = { application_name = "Ardour" }
            quantum = 64
            rate = 96000
        "#;

        let cfg: Config = toml::from_str(sample).expect("parses complete TOML");
        cfg.validate().expect("complete config is valid");

        assert_eq!(cfg.dsp.default_quantum, Some(1024));
        assert_eq!(cfg.pw_settings.min_quantum, Some(64));
        assert_eq!(cfg.pw_settings.max_quantum, Some(2048));
        assert_eq!(
            cfg.pw_settings.allowed_rates.as_deref(),
            Some(&[44100, 48000, 96000][..])
        );
        assert_eq!(cfg.pw_settings.power_of_two_quantum, Some(true));
        assert_eq!(
            cfg.pw_settings.default_audio_sink.as_deref(),
            Some("crimp_game")
        );
        assert_eq!(cfg.virtual_sink.len(), 1);
    }

    #[test]
    fn rejects_invalid_min_max_quantum() {
        let sample = r#"
            [pw_settings]
            min_quantum = 1024
            max_quantum = 256
        "#;
        let cfg: Config = toml::from_str(sample).expect("parses");
        let err = cfg.validate().expect_err("min > max must fail");
        assert!(matches!(err, ConfigError::Validation { .. }));
    }

    #[test]
    fn persists_live_metadata_and_dsp_into_config() {
        let mut cfg = Config::default();
        assert!(
            cfg.apply_metadata_override("clock.force-quantum", Some("256"))
                .expect("quantum")
        );
        assert!(
            cfg.apply_metadata_override("clock.force-rate", Some("96000"))
                .expect("rate")
        );
        assert!(
            cfg.apply_metadata_override("clock.allowed-rates", Some("[ 44100 48000 96000 ]"))
                .expect("allowed-rates")
        );
        assert!(
            cfg.apply_metadata_override("default.audio.sink", Some(r#"{"name":"alsa_test"}"#))
                .expect("default sink")
        );
        assert_eq!(cfg.dsp.default_quantum, Some(256));
        assert_eq!(cfg.dsp.default_rate, Some(96000));
        assert_eq!(
            cfg.pw_settings.allowed_rates.as_deref(),
            Some(&[44100, 48000, 96000][..])
        );
        assert_eq!(
            cfg.pw_settings.default_audio_sink.as_deref(),
            Some("alsa_test")
        );
    }
}
