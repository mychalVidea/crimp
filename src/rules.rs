use crate::config::{Config, NodeMatcher};
use crate::graph::{AudioGraph, NodeInfo, PortInfo, ResolvedLink};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleAction {
    DestroyLink {
        link_id: u32,
        rule_name: String,
        reason: String,
    },
    EnsureRoute {
        source_node_id: u32,
        target_node_id: u32,
        rule_name: String,
    },
}

#[derive(Debug, Clone)]
pub struct RuleEngine {
    config: Config,
}

impl RuleEngine {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn update_config(&mut self, config: Config) {
        self.config = config;
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Evaluates `deny` rules for a newly discovered or resolved link.
    pub fn evaluate_link(&self, resolved: &ResolvedLink<'_>) -> Option<RuleAction> {
        for rule in &self.config.deny {
            if !rule.enabled {
                continue;
            }
            // Both source & target matchers must have at least one constraint across the pair
            if rule.source.is_empty() && rule.target.is_empty() {
                continue;
            }
            let src_ok = matcher_matches_endpoint(&rule.source, resolved.output_node, resolved.output_port);
            let dst_ok = matcher_matches_endpoint(&rule.target, resolved.input_node, resolved.input_port);

            if src_ok && dst_ok {
                let src_label = endpoint_label(resolved.output_node, resolved.link.output_node_id);
                let dst_label = endpoint_label(resolved.input_node, resolved.link.input_node_id);
                return Some(RuleAction::DestroyLink {
                    link_id: resolved.link.id,
                    rule_name: rule.name.clone(),
                    reason: format!("{src_label} -> {dst_label}"),
                });
            }
        }
        None
    }

    /// Prepares future `route` enforcement actions when a matching source node appears.
    pub fn evaluate_node_routes(&self, node: &NodeInfo, graph: &AudioGraph) -> Vec<RuleAction> {
        let mut actions = Vec::new();
        for rule in &self.config.route {
            if !rule.enabled || rule.source.is_empty() || rule.target.is_empty() {
                continue;
            }
            if !matcher_matches_endpoint(&rule.source, Some(node), None) {
                continue;
            }
            let targets = graph.find_nodes_matching(|cand| {
                cand.id != node.id && matcher_matches_endpoint(&rule.target, Some(cand), None)
            });
            for target in targets {
                actions.push(RuleAction::EnsureRoute {
                    source_node_id: node.id,
                    target_node_id: target.id,
                    rule_name: rule.name.clone(),
                });
            }
        }
        actions
    }
}

fn endpoint_label(node: Option<&NodeInfo>, fallback_id: u32) -> String {
    if let Some(n) = node {
        n.app_name
            .as_deref()
            .or(n.node_name.as_deref())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("node#{fallback_id}"))
    } else {
        format!("node#{fallback_id}")
    }
}

pub fn matcher_matches_endpoint(
    matcher: &NodeMatcher,
    node: Option<&NodeInfo>,
    port: Option<&PortInfo>,
) -> bool {
    if let Some(pattern) = &matcher.node_name {
        let actual = node.and_then(|n| n.node_name.as_deref());
        if !actual.is_some_and(|val| wildcard_match_ci(pattern, val)) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.app_name {
        let actual = node.and_then(|n| n.app_name.as_deref());
        if !actual.is_some_and(|val| wildcard_match_ci(pattern, val)) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.media_class {
        let actual = node.and_then(|n| n.media_class.as_deref());
        if !actual.is_some_and(|val| wildcard_match_ci(pattern, val)) {
            return false;
        }
    }
    if let Some(pattern) = &matcher.port_name {
        let actual = port.and_then(|p| p.port_name.as_deref());
        if !actual.is_some_and(|val| wildcard_match_ci(pattern, val)) {
            return false;
        }
    }
    true
}

/// Case-insensitive glob matcher supporting `*`.
pub fn wildcard_match_ci(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;

    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_idx = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(si) = star_idx {
            pi = si + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }

    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DenyRule, RouteRule};
    use crate::graph::LinkInfo;

    #[test]
    fn wildcard_matching_works_case_insensitively() {
        assert!(wildcard_match_ci("obs*", "OBS Studio"));
        assert!(wildcard_match_ci("*virtual-mic*", "crimp_virtual-mic_in"));
        assert!(!wildcard_match_ci("firefox*", "Discord"));
    }

    #[test]
    fn evaluate_link_triggers_destroy_on_deny_match() {
        let cfg = Config {
            deny: vec![DenyRule {
                name: "block-obs-mic".into(),
                enabled: true,
                source: NodeMatcher {
                    app_name: Some("OBS*".into()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    node_name: Some("*virtual-mic*".into()),
                    ..Default::default()
                },
            }],
            route: vec![],
        };
        let engine = RuleEngine::new(cfg);

        let src = NodeInfo {
            id: 1,
            node_name: Some("obs-audio".into()),
            app_name: Some("OBS Studio".into()),
            media_class: Some("Stream/Output/Audio".into()),
        };
        let dst = NodeInfo {
            id: 2,
            node_name: Some("my-virtual-mic".into()),
            app_name: None,
            media_class: Some("Audio/Sink".into()),
        };
        let link = LinkInfo {
            id: 42,
            output_node_id: 1,
            output_port_id: 10,
            input_node_id: 2,
            input_port_id: 20,
        };
        let resolved = ResolvedLink {
            link: &link,
            output_node: Some(&src),
            output_port: None,
            input_node: Some(&dst),
            input_port: None,
        };

        assert_eq!(
            engine.evaluate_link(&resolved),
            Some(RuleAction::DestroyLink {
                link_id: 42,
                rule_name: "block-obs-mic".into(),
                reason: "OBS Studio -> my-virtual-mic".into(),
            })
        );
    }

    #[test]
    fn evaluate_node_routes_prepares_ensure_route() {
        let cfg = Config {
            deny: vec![],
            route: vec![RouteRule {
                name: "discord-comms".into(),
                enabled: true,
                source: NodeMatcher {
                    app_name: Some("Discord*".into()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    node_name: Some("crimp-comms-sink".into()),
                    ..Default::default()
                },
            }],
        };
        let engine = RuleEngine::new(cfg);
        let mut graph = AudioGraph::new();
        let src = NodeInfo {
            id: 5,
            node_name: Some("discord-out".into()),
            app_name: Some("Discord".into()),
            media_class: Some("Stream/Output/Audio".into()),
        };
        let sink = NodeInfo {
            id: 8,
            node_name: Some("crimp-comms-sink".into()),
            app_name: None,
            media_class: Some("Audio/Sink".into()),
        };
        graph.upsert_node(src.clone());
        graph.upsert_node(sink);

        let actions = engine.evaluate_node_routes(&src, &graph);
        assert_eq!(
            actions,
            vec![RuleAction::EnsureRoute {
                source_node_id: 5,
                target_node_id: 8,
                rule_name: "discord-comms".into(),
            }]
        );
    }
}
