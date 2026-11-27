use crate::config::{Config, ConfigError, load_existing, save_config};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const SCENE_NAME_PREFIX: &str = "# crimp-scene-name:";

/// Metadata jedné uložené scény (presetu celého `rules.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub quantum: Option<u32>,
    pub rate: Option<u32>,
    pub route_count: usize,
    pub deny_count: usize,
    pub dsp_profile_count: usize,
}

/// Rychlá výchozí šablona scény pro 1-klik vytvoření v UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterSceneKind {
    Gaming,
    Studio96k,
    AutoBalanced,
}

impl StarterSceneKind {
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Gaming => "Hraní (512 @ 48 kHz)",
            Self::Studio96k => "Hudba / Studio (512 @ 96 kHz)",
            Self::AutoBalanced => "Auto / Úsporný",
        }
    }

    #[must_use]
    pub fn apply_to(self, base: &Config) -> Config {
        let mut next = base.clone();
        match self {
            Self::Gaming => {
                next.dsp.default_quantum = Some(512);
                next.dsp.default_rate = Some(48_000);
            }
            Self::Studio96k => {
                next.dsp.default_quantum = Some(512);
                next.dsp.default_rate = Some(96_000);
                if next.pw_settings.allowed_rates.is_none() {
                    next.pw_settings.allowed_rates = Some(vec![44_100, 48_000, 96_000]);
                }
            }
            Self::AutoBalanced => {
                next.dsp.default_quantum = None;
                next.dsp.default_rate = None;
            }
        }
        next
    }
}

/// Výchozí složka uložených scén: `~/.config/crimp/scenes/`.
pub fn default_scenes_dir() -> Result<PathBuf, ConfigError> {
    let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.join("crimp").join("scenes"))
}

/// Převede libovolný název scény na bezpečný název souboru (`slug`).
#[must_use]
pub fn slugify_scene_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in name.trim().chars() {
        let mapped = match ch {
            'á' | 'Á' => Some('a'),
            'č' | 'Č' => Some('c'),
            'ď' | 'Ď' => Some('d'),
            'é' | 'É' | 'ě' | 'Ě' => Some('e'),
            'í' | 'Í' => Some('i'),
            'ň' | 'Ň' => Some('n'),
            'ó' | 'Ó' => Some('o'),
            'ř' | 'Ř' => Some('r'),
            'š' | 'Š' => Some('s'),
            'ť' | 'Ť' => Some('t'),
            'ú' | 'Ú' | 'ů' | 'Ů' => Some('u'),
            'ý' | 'Ý' => Some('y'),
            'ž' | 'Ž' => Some('z'),
            c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        if let Some(c) = mapped {
            slug.push(c);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "scena".to_string()
    } else {
        trimmed
    }
}

#[must_use]
pub fn list_scenes() -> Vec<SceneInfo> {
    match default_scenes_dir() {
        Ok(dir) => list_scenes_in(&dir),
        Err(_) => Vec::new(),
    }
}

#[must_use]
pub fn list_scenes_in(dir: &Path) -> Vec<SceneInfo> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut scenes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok((name, cfg)) = read_scene_file(&path, stem) {
            scenes.push(SceneInfo {
                id: stem.to_string(),
                name,
                path,
                quantum: cfg.dsp.default_quantum,
                rate: cfg.dsp.default_rate,
                route_count: cfg.route.len(),
                deny_count: cfg.deny.len(),
                dsp_profile_count: cfg.dsp_profile.len(),
            });
        }
    }
    scenes.sort_by_key(|a| a.name.to_lowercase());
    scenes
}

pub fn save_scene(name: &str, cfg: &Config) -> Result<SceneInfo, ConfigError> {
    let dir = default_scenes_dir()?;
    save_scene_in(&dir, name, cfg)
}

pub fn save_scene_in(dir: &Path, name: &str, cfg: &Config) -> Result<SceneInfo, ConfigError> {
    let display_name = name.trim();
    if display_name.is_empty() {
        return Err(ConfigError::Validation {
            rule: "scene".into(),
            reason: "Název scény nesmí být prázdný".into(),
        });
    }
    let id = slugify_scene_name(display_name);
    let path = dir.join(format!("{id}.toml"));
    save_config(&path, cfg)?;

    // Připojíme hlavičku s čitelným názvem scény (TOML komentář), aby diakritika přežila.
    let existing = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    let with_meta = format!("{SCENE_NAME_PREFIX} {display_name}\n{existing}");
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, with_meta).map_err(|source| ConfigError::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, &path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(SceneInfo {
        id,
        name: display_name.to_string(),
        path,
        quantum: cfg.dsp.default_quantum,
        rate: cfg.dsp.default_rate,
        route_count: cfg.route.len(),
        deny_count: cfg.deny.len(),
        dsp_profile_count: cfg.dsp_profile.len(),
    })
}

pub fn load_scene(id: &str) -> Result<(String, Config), ConfigError> {
    let dir = default_scenes_dir()?;
    load_scene_in(&dir, id)
}

pub fn load_scene_in(dir: &Path, id: &str) -> Result<(String, Config), ConfigError> {
    let clean_id = slugify_scene_name(id);
    let path = dir.join(format!("{clean_id}.toml"));
    read_scene_file(&path, &clean_id)
}

pub fn delete_scene(id: &str) -> Result<(), ConfigError> {
    let dir = default_scenes_dir()?;
    delete_scene_in(&dir, id)
}

pub fn delete_scene_in(dir: &Path, id: &str) -> Result<(), ConfigError> {
    let clean_id = slugify_scene_name(id);
    let path = dir.join(format!("{clean_id}.toml"));
    if path.exists() {
        fs::remove_file(&path).map_err(|source| ConfigError::Io { path, source })?;
    }
    Ok(())
}

fn read_scene_file(path: &Path, fallback_name: &str) -> Result<(String, Config), ConfigError> {
    let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let name = raw
        .lines()
        .take(3)
        .find_map(|line| {
            line.trim()
                .strip_prefix(SCENE_NAME_PREFIX)
                .map(|rest| rest.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| fallback_name.to_string());
    let cfg = load_existing(path)?;
    Ok((name, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn slugifies_czech_names_cleanly() {
        assert_eq!(slugify_scene_name("Hraní / CS2!"), "hrani-cs2");
        assert_eq!(slugify_scene_name("Hudba 96 kHz"), "hudba-96-khz");
        assert_eq!(slugify_scene_name("   "), "scena");
    }

    #[test]
    fn scene_store_save_list_load_delete_roundtrip() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("crimp-scenes-test-{nonce}"));

        let mut cfg = Config::default();
        cfg.dsp.default_quantum = Some(512);
        cfg.dsp.default_rate = Some(96_000);

        let saved = save_scene_in(&dir, "Stream / OBS", &cfg).expect("save scene");
        assert_eq!(saved.id, "stream-obs");
        assert_eq!(saved.name, "Stream / OBS");
        assert_eq!(saved.quantum, Some(512));
        assert_eq!(saved.rate, Some(96_000));

        let listed = list_scenes_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Stream / OBS");

        let (loaded_name, loaded_cfg) = load_scene_in(&dir, "stream-obs").expect("load scene");
        assert_eq!(loaded_name, "Stream / OBS");
        assert_eq!(loaded_cfg, cfg);

        delete_scene_in(&dir, "stream-obs").expect("delete scene");
        assert!(list_scenes_in(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
