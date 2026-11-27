use crimp_proto::graph::NodeSnapshot;
use egui::{Pos2, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

/// Uložený stav uživatelského rozvržení Patchbay plátna (`~/.config/crimp/ui-layout.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedLayout {
    pub node_positions: BTreeMap<String, [f32; 2]>,
    pub pan: [f32; 2],
    pub zoom: f32,
    pub hide_internal_and_midi: bool,
    #[serde(default)]
    pub touchpad_pan: bool,
}

impl Default for SavedLayout {
    fn default() -> Self {
        Self {
            node_positions: BTreeMap::new(),
            pan: [40.0, 40.0],
            zoom: 1.0,
            hide_internal_and_midi: true,
            touchpad_pan: false,
        }
    }
}

#[must_use]
pub fn layout_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("crimp").join("ui-layout.json"))
}

#[must_use]
pub fn load_saved_layout() -> SavedLayout {
    let Some(path) = layout_file_path() else {
        return SavedLayout::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return SavedLayout::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_layout(layout: &SavedLayout) {
    let Some(path) = layout_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(layout) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Vytvoří stabilní mapu `node_id -> perzistentní klíč`, který vydrží i po restartu aplikací
/// (přestože PipeWire při každém restartu přidělí nové číselné ID).
#[must_use]
pub fn compute_stable_keys(nodes: &[&NodeSnapshot]) -> HashMap<u32, String> {
    let mut ordered: Vec<&NodeSnapshot> = nodes.to_vec();
    ordered.sort_by_key(|n| n.id);

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut result = HashMap::with_capacity(ordered.len());

    for node in ordered {
        let base = format!(
            "{}|{}|{}",
            node.node_name.as_deref().unwrap_or(""),
            node.application_name.as_deref().unwrap_or(""),
            node.media_class.as_deref().unwrap_or("")
        );
        let idx = counts.entry(base.clone()).or_insert(0);
        let key = format!("{base}#{idx}");
        *idx += 1;
        result.insert(node.id, key);
    }

    result
}

#[must_use]
pub fn pos_to_arr(p: Pos2) -> [f32; 2] {
    [p.x, p.y]
}

#[must_use]
pub fn arr_to_pos(a: [f32; 2]) -> Pos2 {
    Pos2::new(a[0], a[1])
}

#[must_use]
pub fn vec_to_arr(v: Vec2) -> [f32; 2] {
    [v.x, v.y]
}

#[must_use]
pub fn arr_to_vec(a: [f32; 2]) -> Vec2 {
    Vec2::new(a[0], a[1])
}
