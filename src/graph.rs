use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: u32,
    pub node_name: Option<String>,
    pub app_name: Option<String>,
    pub media_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub id: u32,
    pub node_id: Option<u32>,
    pub port_name: Option<String>,
    pub direction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub id: u32,
    pub output_node_id: u32,
    pub output_port_id: u32,
    pub input_node_id: u32,
    pub input_port_id: u32,
}

#[derive(Debug, Clone)]
pub struct ResolvedLink<'a> {
    pub link: &'a LinkInfo,
    pub output_node: Option<&'a NodeInfo>,
    pub output_port: Option<&'a PortInfo>,
    pub input_node: Option<&'a NodeInfo>,
    pub input_port: Option<&'a PortInfo>,
}

#[derive(Debug, Default)]
pub struct AudioGraph {
    nodes: HashMap<u32, NodeInfo>,
    ports: HashMap<u32, PortInfo>,
    links: HashMap<u32, LinkInfo>,
}

impl AudioGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_node(&mut self, node: NodeInfo) {
        self.nodes.insert(node.id, node);
    }

    pub fn upsert_port(&mut self, port: PortInfo) {
        self.ports.insert(port.id, port);
    }

    pub fn upsert_link(&mut self, link: LinkInfo) {
        self.links.insert(link.id, link);
    }

    pub fn remove_global(&mut self, id: u32) {
        self.nodes.remove(&id);
        self.ports.remove(&id);
        self.links.remove(&id);
    }

    pub fn get_node(&self, id: u32) -> Option<&NodeInfo> {
        self.nodes.get(&id)
    }

    pub fn get_port(&self, id: u32) -> Option<&PortInfo> {
        self.ports.get(&id)
    }

    pub fn get_link(&self, id: u32) -> Option<&LinkInfo> {
        self.links.get(&id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &NodeInfo> {
        self.nodes.values()
    }

    pub fn all_link_ids(&self) -> Vec<u32> {
        self.links.keys().copied().collect()
    }

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

    pub fn find_nodes_matching<F>(&self, mut predicate: F) -> Vec<&NodeInfo>
    where
        F: FnMut(&NodeInfo) -> bool,
    {
        self.nodes.values().filter(|n| predicate(n)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_link_endpoints_by_id() {
        let mut graph = AudioGraph::new();
        graph.upsert_node(NodeInfo {
            id: 10,
            node_name: Some("obs-capture".into()),
            app_name: Some("OBS".into()),
            media_class: Some("Stream/Output/Audio".into()),
        });
        graph.upsert_node(NodeInfo {
            id: 20,
            node_name: Some("alsa_output.pci".into()),
            app_name: None,
            media_class: Some("Audio/Sink".into()),
        });
        graph.upsert_port(PortInfo {
            id: 11,
            node_id: Some(10),
            port_name: Some("output_FL".into()),
            direction: Some("out".into()),
        });
        graph.upsert_port(PortInfo {
            id: 21,
            node_id: Some(20),
            port_name: Some("playback_FL".into()),
            direction: Some("in".into()),
        });
        graph.upsert_link(LinkInfo {
            id: 99,
            output_node_id: 10,
            output_port_id: 11,
            input_node_id: 20,
            input_port_id: 21,
        });

        let resolved = graph.resolve_link(99).expect("link exists");
        assert_eq!(resolved.output_node.unwrap().app_name.as_deref(), Some("OBS"));
        assert_eq!(resolved.input_node.unwrap().node_name.as_deref(), Some("alsa_output.pci"));
        assert_eq!(resolved.output_port.unwrap().port_name.as_deref(), Some("output_FL"));
        assert_eq!(resolved.input_port.unwrap().port_name.as_deref(), Some("playback_FL"));

        graph.remove_global(99);
        assert!(graph.resolve_link(99).is_none());
    }
}
