use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Směr audio/MIDI portu v PipeWire grafu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortDirection {
    Output,
    Input,
    Unknown,
}

impl PortDirection {
    #[must_use]
    pub fn from_pw_str(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("out") => Self::Output,
            Some("in") => Self::Input,
            _ => Self::Unknown,
        }
    }
}

/// Metadata uzlu v PipeWire grafu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: u32,
    pub node_name: Option<String>,
    pub node_description: Option<String>,
    pub application_name: Option<String>,
    pub media_class: Option<String>,
    #[serde(default)]
    pub node_latency: Option<String>,
    #[serde(default)]
    pub node_rate: Option<String>,
}

impl NodeInfo {
    #[must_use]
    pub fn display_label(&self) -> String {
        if let Some(app) = &self.application_name {
            return match &self.node_name {
                Some(name) if name != app => format!("{app} ({name})"),
                _ => app.clone(),
            };
        }
        if let Some(desc) = &self.node_description {
            return desc.clone();
        }
        if let Some(name) = &self.node_name {
            return name.clone();
        }
        format!("node#{}", self.id)
    }
}

/// Metadata portu v PipeWire grafu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub id: u32,
    pub node_id: u32,
    pub port_name: Option<String>,
    pub direction: PortDirection,
    pub channel: Option<String>,
}

/// Metadata propojení (linku) mezi dvěma porty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub id: u32,
    pub output_node_id: u32,
    pub output_port_id: u32,
    pub input_node_id: u32,
    pub input_port_id: u32,
}

/// Plně rozlišený link včetně obou koncových uzlů a portů pro vyhodnocování pravidel.
#[derive(Debug, Clone)]
pub struct ResolvedLink<'a> {
    pub link: &'a LinkInfo,
    pub output_node: Option<&'a NodeInfo>,
    pub output_port: Option<&'a PortInfo>,
    pub input_node: Option<&'a NodeInfo>,
    pub input_port: Option<&'a PortInfo>,
}

/// Záznam o zablokovaném linku pro historii v IPC/CLI/GUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeniedRecord {
    pub timestamp_epoch_ms: u128,
    pub rule_name: String,
    pub link_id: u32,
    pub source_label: String,
    pub target_label: String,
}

/// Živý přehled PipeWire Core a Metadata (`settings` + `default`) vlastností.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PwRuntimeState {
    pub core_name: Option<String>,
    pub core_version: Option<String>,
    pub user_name: Option<String>,
    pub host_name: Option<String>,
    pub clock_rate: Option<String>,
    pub clock_force_rate: Option<String>,
    pub clock_allowed_rates: Option<String>,
    pub clock_quantum: Option<String>,
    pub clock_force_quantum: Option<String>,
    pub clock_min_quantum: Option<String>,
    pub clock_max_quantum: Option<String>,
    pub clock_quantum_limit: Option<String>,
    pub clock_quantum_floor: Option<String>,
    pub clock_power_of_two: Option<String>,
    pub log_level: Option<String>,
    pub default_audio_sink: Option<String>,
    pub default_audio_source: Option<String>,
    pub default_configured_sink: Option<String>,
    pub default_configured_source: Option<String>,
    pub raw_settings_metadata: BTreeMap<String, String>,
    pub raw_default_metadata: BTreeMap<String, String>,
    pub raw_core_props: BTreeMap<String, String>,
}

impl PwRuntimeState {
    /// Z aktualních/vynucených hodnot hodin vypočítá `(quantum, rate, latency_ms)`.
    #[must_use]
    pub fn effective_latency(&self) -> Option<(u32, u32, f64)> {
        let q = parse_positive_pw_u32(self.clock_force_quantum.as_deref())
            .or_else(|| parse_positive_pw_u32(self.clock_quantum.as_deref()))
            .or_else(|| {
                self.raw_core_props
                    .get("default.clock.quantum")
                    .and_then(|v| parse_positive_pw_u32(Some(v)))
            })?;

        let r = parse_positive_pw_u32(self.clock_force_rate.as_deref())
            .or_else(|| parse_positive_pw_u32(self.clock_rate.as_deref()))
            .or_else(|| {
                self.raw_core_props
                    .get("default.clock.rate")
                    .and_then(|v| parse_positive_pw_u32(Some(v)))
            })?;

        let ms = (f64::from(q) / f64::from(r)) * 1000.0;
        Some((q, r, ms))
    }

    /// Extrahuje jméno defaultního audio sinku/zdroje ze PipeWire JSON řetězce (`{"name":"..."}`) nebo prostého názvu.
    #[must_use]
    pub fn extract_device_name(raw: Option<&str>) -> Option<String> {
        let raw = raw?.trim();
        if raw.is_empty() {
            return None;
        }
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw)
            && let Some(serde_json::Value::String(name)) = map.get("name")
        {
            return Some(name.clone());
        }
        Some(raw.to_string())
    }
}

fn parse_positive_pw_u32(raw: Option<&str>) -> Option<u32> {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
}

/// Serializovatelný snímek celého audio grafu pro IPC (`crimp graph` / `crimp-ui`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub nodes: Vec<NodeSnapshot>,
    pub links: Vec<LinkSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub id: u32,
    pub label: String,
    pub node_name: Option<String>,
    pub node_description: Option<String>,
    pub application_name: Option<String>,
    pub media_class: Option<String>,
    pub node_latency: Option<String>,
    pub node_rate: Option<String>,
    pub is_virtual_crimp: bool,
    pub output_ports: Vec<PortInfo>,
    pub input_ports: Vec<PortInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSnapshot {
    pub id: u32,
    pub output_node_id: u32,
    pub output_port_id: u32,
    pub output_label: String,
    pub input_node_id: u32,
    pub input_port_id: u32,
    pub input_label: String,
    /// Pokud link odpovídá některému zapnutému `[[route]]` pravidlu, nese jeho název.
    pub enforced_by_rule: Option<String>,
}

/// In-memory stavová mapa aktivního PipeWire grafu.
#[derive(Debug, Default)]
pub struct AudioGraph {
    nodes: BTreeMap<u32, NodeInfo>,
    ports: BTreeMap<u32, PortInfo>,
    links: BTreeMap<u32, LinkInfo>,
}

impl AudioGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_node(&mut self, info: NodeInfo) {
        self.nodes.insert(info.id, info);
    }

    pub fn upsert_port(&mut self, info: PortInfo) {
        self.ports.insert(info.id, info);
    }

    pub fn upsert_link(&mut self, info: LinkInfo) {
        self.links.insert(info.id, info);
    }

    pub fn remove_global(&mut self, id: u32) -> bool {
        if self.nodes.remove(&id).is_some() {
            self.ports.retain(|_, port| port.node_id != id);
            self.links
                .retain(|_, link| link.output_node_id != id && link.input_node_id != id);
            return true;
        }
        if self.ports.remove(&id).is_some() {
            self.links
                .retain(|_, link| link.output_port_id != id && link.input_port_id != id);
            return true;
        }
        self.links.remove(&id).is_some()
    }

    #[must_use]
    pub fn node(&self, id: u32) -> Option<&NodeInfo> {
        self.nodes.get(&id)
    }

    #[must_use]
    pub fn port(&self, id: u32) -> Option<&PortInfo> {
        self.ports.get(&id)
    }

    #[must_use]
    pub fn link(&self, id: u32) -> Option<&LinkInfo> {
        self.links.get(&id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &NodeInfo> {
        self.nodes.values()
    }

    pub fn ports(&self) -> impl Iterator<Item = &PortInfo> {
        self.ports.values()
    }

    pub fn links(&self) -> impl Iterator<Item = &LinkInfo> {
        self.links.values()
    }

    #[must_use]
    pub fn ports_for_node(&self, node_id: u32, direction: PortDirection) -> Vec<&PortInfo> {
        self.ports
            .values()
            .filter(|port| port.node_id == node_id && port.direction == direction)
            .collect()
    }

    #[must_use]
    pub fn find_link_between_ports(&self, output_port_id: u32, input_port_id: u32) -> Option<u32> {
        self.links
            .values()
            .find(|link| {
                link.output_port_id == output_port_id && link.input_port_id == input_port_id
            })
            .map(|link| link.id)
    }

    #[must_use]
    pub fn links_from_node(&self, node_id: u32) -> Vec<&LinkInfo> {
        self.links
            .values()
            .filter(|link| link.output_node_id == node_id)
            .collect()
    }

    #[must_use]
    pub fn resolve_link(&self, link_id: u32) -> Option<ResolvedLink<'_>> {
        let link = self.links.get(&link_id)?;
        Some(ResolvedLink {
            link,
            output_node: self.nodes.get(&link.output_node_id),
            output_port: self.ports.get(&link.output_port_id),
            input_node: self.nodes.get(&link.input_node_id),
            input_port: self.ports.get(&link.input_port_id),
        })
    }

    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.nodes.len(), self.ports.len(), self.links.len())
    }

    /// Vytvoří JSON-ready snímek celého grafu s označenými virtuálními sinky a vynucenými linky.
    #[must_use]
    pub fn snapshot_with_annotations<F>(
        &self,
        virtual_sink_names: &HashSet<String>,
        route_lookup: F,
    ) -> GraphSnapshot
    where
        F: Fn(&ResolvedLink<'_>) -> Option<String>,
    {
        let nodes = self
            .nodes
            .values()
            .map(|node| {
                let output_ports = self
                    .ports_for_node(node.id, PortDirection::Output)
                    .into_iter()
                    .cloned()
                    .collect();
                let input_ports = self
                    .ports_for_node(node.id, PortDirection::Input)
                    .into_iter()
                    .cloned()
                    .collect();
                let is_virtual_crimp = node
                    .node_name
                    .as_ref()
                    .is_some_and(|name| virtual_sink_names.contains(name));
                NodeSnapshot {
                    id: node.id,
                    label: node.display_label(),
                    node_name: node.node_name.clone(),
                    node_description: node.node_description.clone(),
                    application_name: node.application_name.clone(),
                    media_class: node.media_class.clone(),
                    node_latency: node.node_latency.clone(),
                    node_rate: node.node_rate.clone(),
                    is_virtual_crimp,
                    output_ports,
                    input_ports,
                }
            })
            .collect();

        let links = self
            .links
            .values()
            .map(|link| {
                let enforced_by_rule = self
                    .resolve_link(link.id)
                    .and_then(|resolved| route_lookup(&resolved));
                LinkSnapshot {
                    id: link.id,
                    output_node_id: link.output_node_id,
                    output_port_id: link.output_port_id,
                    output_label: self.nodes.get(&link.output_node_id).map_or_else(
                        || format!("node#{}", link.output_node_id),
                        NodeInfo::display_label,
                    ),
                    input_node_id: link.input_node_id,
                    input_port_id: link.input_port_id,
                    input_label: self.nodes.get(&link.input_node_id).map_or_else(
                        || format!("node#{}", link.input_node_id),
                        NodeInfo::display_label,
                    ),
                    enforced_by_rule,
                }
            })
            .collect();

        GraphSnapshot { nodes, links }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_cascade_cleanup_and_latency_calc() {
        let mut graph = AudioGraph::new();
        graph.upsert_node(NodeInfo {
            id: 10,
            node_name: Some("src".into()),
            node_description: None,
            application_name: Some("App".into()),
            media_class: Some("Stream/Output/Audio".into()),
            node_latency: Some("128/48000".into()),
            node_rate: Some("1/48000".into()),
        });
        graph.upsert_port(PortInfo {
            id: 11,
            node_id: 10,
            port_name: Some("output_FL".into()),
            direction: PortDirection::Output,
            channel: Some("FL".into()),
        });
        graph.upsert_node(NodeInfo {
            id: 20,
            node_name: Some("sink".into()),
            node_description: None,
            application_name: None,
            media_class: Some("Audio/Sink".into()),
            node_latency: None,
            node_rate: None,
        });
        graph.upsert_port(PortInfo {
            id: 21,
            node_id: 20,
            port_name: Some("playback_FL".into()),
            direction: PortDirection::Input,
            channel: Some("FL".into()),
        });
        graph.upsert_link(LinkInfo {
            id: 30,
            output_node_id: 10,
            output_port_id: 11,
            input_node_id: 20,
            input_port_id: 21,
        });

        assert_eq!(graph.find_link_between_ports(11, 21), Some(30));
        assert!(graph.remove_global(10));
        assert_eq!(graph.counts(), (1, 1, 0));

        let rt = PwRuntimeState {
            clock_quantum: Some("1024".into()),
            clock_force_quantum: Some("128".into()),
            clock_rate: Some("48000".into()),
            ..Default::default()
        };
        let (q, r, ms) = rt.effective_latency().expect("latency");
        assert_eq!((q, r), (128, 48000));
        assert!((ms - 2.666_666_6).abs() < 0.01);
        assert_eq!(
            PwRuntimeState::extract_device_name(Some(r#"{"name":"alsa_output.test"}"#)).as_deref(),
            Some("alsa_output.test")
        );
    }
}
