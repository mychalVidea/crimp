use crate::config::{Config, DenyRule, NodeMatcher, RouteRule};
use crate::graph::{AudioGraph, LinkInfo, NodeInfo, PortDirection, PortInfo, ResolvedLink};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyAction {
    DestroyLink { link_id: u32, rule_name: String },
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAction {
    DestroyCompetingLink {
        link_id: u32,
        rule_name: String,
    },
    CreateLink {
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
        rule_name: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DspTarget {
    pub quantum: Option<u32>,
    pub rate: Option<u32>,
    pub active_profile: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuleEngine {
    config: Config,
}

impl RuleEngine {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn update_config(&mut self, config: Config) {
        self.config = config;
    }

    #[must_use]
    pub fn evaluate_link(&self, link: &ResolvedLink<'_>) -> DenyAction {
        if let Some(rule) = self.find_matching_deny(
            link.output_node,
            link.output_port,
            link.input_node,
            link.input_port,
        ) {
            return DenyAction::DestroyLink {
                link_id: link.link.id,
                rule_name: rule.name.clone(),
            };
        }
        DenyAction::Allow
    }

    /// Zjistí, zda daný ručně vytvářený nebo existující link spadá pod nějaké `[[deny]]` pravidlo.
    #[must_use]
    pub fn find_deny_for_endpoints(
        &self,
        output_node: Option<&NodeInfo>,
        output_port: Option<&PortInfo>,
        input_node: Option<&NodeInfo>,
        input_port: Option<&PortInfo>,
    ) -> Option<&DenyRule> {
        self.find_matching_deny(output_node, output_port, input_node, input_port)
    }

    /// Vrátí název zapnutého `[[route]]` pravidla, které odpovídá existujícímu linku.
    #[must_use]
    pub fn matching_route_for_link(&self, link: &ResolvedLink<'_>) -> Option<String> {
        self.config
            .route
            .iter()
            .filter(|rule| rule.enabled)
            .find(|rule| {
                matcher_matches_endpoint(&rule.source, link.output_node, link.output_port)
                    && matcher_matches_endpoint(&rule.target, link.input_node, link.input_port)
            })
            .map(|rule| rule.name.clone())
    }

    /// Vrátí uživatelský alias uzlu, pokud odpovídá některému pravidlu `[[alias]]`.
    #[must_use]
    pub fn resolve_alias(&self, node: &NodeInfo) -> Option<String> {
        self.config
            .alias
            .iter()
            .rfind(|a| matcher_matches_node(&a.matcher, node))
            .map(|a| a.display_name.clone())
    }

    fn find_matching_deny(
        &self,
        output_node: Option<&NodeInfo>,
        output_port: Option<&PortInfo>,
        input_node: Option<&NodeInfo>,
        input_port: Option<&PortInfo>,
    ) -> Option<&DenyRule> {
        self.config
            .deny
            .iter()
            .filter(|rule| rule.enabled)
            .find(|rule| {
                matcher_matches_endpoint(&rule.source, output_node, output_port)
                    && matcher_matches_endpoint(&rule.target, input_node, input_port)
            })
    }

    #[must_use]
    pub fn plan_routes(&self, graph: &AudioGraph) -> Vec<RouteAction> {
        let mut actions = Vec::new();
        let mut planned_destroys = HashSet::new();
        let mut planned_creates = HashSet::new();

        for rule in self.config.route.iter().filter(|r| r.enabled) {
            self.plan_single_route(
                rule,
                graph,
                &mut planned_destroys,
                &mut planned_creates,
                &mut actions,
            );
        }

        actions
    }

    fn plan_single_route(
        &self,
        rule: &RouteRule,
        graph: &AudioGraph,
        planned_destroys: &mut HashSet<u32>,
        planned_creates: &mut HashSet<(u32, u32)>,
        actions: &mut Vec<RouteAction>,
    ) {
        let candidate_targets: Vec<&NodeInfo> = graph
            .nodes()
            .filter(|node| matcher_matches_node(&rule.target, node))
            .collect();

        if candidate_targets.is_empty() {
            return;
        }

        for source_node in graph.nodes() {
            if !matcher_matches_node(&rule.source, source_node) {
                continue;
            }

            let Some(target_node) = candidate_targets
                .iter()
                .copied()
                .find(|target| target.id != source_node.id)
            else {
                continue;
            };

            let out_ports =
                filtered_ports(graph, source_node.id, PortDirection::Output, &rule.source);
            let in_ports =
                filtered_ports(graph, target_node.id, PortDirection::Input, &rule.target);
            if out_ports.is_empty() || in_ports.is_empty() {
                continue;
            }

            let out_port_ids: HashSet<u32> = out_ports.iter().map(|p| p.id).collect();

            if rule.exclusive {
                for existing in graph.links_from_node(source_node.id) {
                    if !out_port_ids.contains(&existing.output_port_id) {
                        continue;
                    }
                    if !is_link_on_rule_target(existing, graph, &rule.target)
                        && planned_destroys.insert(existing.id)
                    {
                        actions.push(RouteAction::DestroyCompetingLink {
                            link_id: existing.id,
                            rule_name: rule.name.clone(),
                        });
                    }
                }
            }

            for (out_port, in_port) in pair_ports(&out_ports, &in_ports) {
                if graph
                    .find_link_between_ports(out_port.id, in_port.id)
                    .is_some()
                {
                    continue;
                }

                if self
                    .find_matching_deny(
                        Some(source_node),
                        Some(out_port),
                        Some(target_node),
                        Some(in_port),
                    )
                    .is_some()
                {
                    continue;
                }

                if planned_creates.insert((out_port.id, in_port.id)) {
                    actions.push(RouteAction::CreateLink {
                        output_node_id: source_node.id,
                        output_port_id: out_port.id,
                        input_node_id: target_node.id,
                        input_port_id: in_port.id,
                        rule_name: rule.name.clone(),
                    });
                }
            }
        }
    }

    #[must_use]
    pub fn evaluate_dsp(
        &self,
        graph: &AudioGraph,
        override_quantum: Option<u32>,
        override_rate: Option<u32>,
    ) -> DspTarget {
        let best_profile = self
            .config
            .dsp_profile
            .iter()
            .enumerate()
            .filter(|(_, profile)| profile.enabled)
            .filter(|(_, profile)| {
                graph
                    .nodes()
                    .any(|node| matcher_matches_node(&profile.source, node))
            })
            .max_by(|(idx_a, a), (idx_b, b)| {
                a.priority.cmp(&b.priority).then_with(|| idx_b.cmp(idx_a))
            })
            .map(|(_, profile)| profile);

        let mut target = DspTarget {
            quantum: best_profile
                .and_then(|p| p.quantum)
                .or(self.config.dsp.default_quantum),
            rate: best_profile
                .and_then(|p| p.rate)
                .or(self.config.dsp.default_rate),
            active_profile: best_profile.map(|p| p.name.clone()),
        };

        if let Some(q) = override_quantum {
            target.quantum = Some(q);
            target.active_profile = Some("manual-override".to_string());
        }
        if let Some(r) = override_rate {
            target.rate = Some(r);
            if target.active_profile.is_none() || override_quantum.is_none() {
                target.active_profile = Some("manual-override".to_string());
            }
        }

        target
    }
}

fn filtered_ports<'a>(
    graph: &'a AudioGraph,
    node_id: u32,
    direction: PortDirection,
    matcher: &NodeMatcher,
) -> Vec<&'a PortInfo> {
    graph
        .ports_for_node(node_id, direction)
        .into_iter()
        .filter(|port| matches_opt(&matcher.port_name, port.port_name.as_deref()))
        .collect()
}

fn is_link_on_rule_target(
    link: &LinkInfo,
    graph: &AudioGraph,
    target_matcher: &NodeMatcher,
) -> bool {
    let target_node = graph.node(link.input_node_id);
    let target_port = graph.port(link.input_port_id);
    matcher_matches_endpoint(target_matcher, target_node, target_port)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CanonicalChannel {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    RearLeft,
    RearRight,
    SideLeft,
    SideRight,
    Mono,
    Named(String),
}

fn port_channel(port: &PortInfo) -> Option<CanonicalChannel> {
    let raw = port
        .channel
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            port.port_name
                .as_deref()
                .and_then(|name| name.rsplit('_').next())
        })?;

    let upper = raw.trim().to_ascii_uppercase();
    let ch = match upper.as_str() {
        "FL" | "L" | "LEFT" | "AUX0" => CanonicalChannel::FrontLeft,
        "FR" | "R" | "RIGHT" | "AUX1" => CanonicalChannel::FrontRight,
        "FC" | "C" | "CENTER" | "AUX2" => CanonicalChannel::FrontCenter,
        "LFE" | "SUB" => CanonicalChannel::Lfe,
        "RL" | "BL" => CanonicalChannel::RearLeft,
        "RR" | "BR" => CanonicalChannel::RearRight,
        "SL" => CanonicalChannel::SideLeft,
        "SR" => CanonicalChannel::SideRight,
        "MONO" | "M" => CanonicalChannel::Mono,
        other => CanonicalChannel::Named(other.to_string()),
    };
    Some(ch)
}

fn pair_ports<'a>(
    out_ports: &[&'a PortInfo],
    in_ports: &[&'a PortInfo],
) -> Vec<(&'a PortInfo, &'a PortInfo)> {
    if out_ports.len() == 1
        && port_channel(out_ports[0]) == Some(CanonicalChannel::Mono)
        && in_ports.len() > 1
    {
        return in_ports
            .iter()
            .filter(|in_p| {
                matches!(
                    port_channel(in_p),
                    Some(CanonicalChannel::FrontLeft | CanonicalChannel::FrontRight)
                )
            })
            .map(|&in_p| (out_ports[0], in_p))
            .collect();
    }

    let mut pairs = Vec::new();
    let mut used_in = HashSet::new();

    for &out_p in out_ports {
        let Some(out_ch) = port_channel(out_p) else {
            continue;
        };
        if let Some(&in_p) = in_ports
            .iter()
            .find(|&&p| !used_in.contains(&p.id) && port_channel(p).as_ref() == Some(&out_ch))
        {
            used_in.insert(in_p.id);
            pairs.push((out_p, in_p));
        }
    }

    if !pairs.is_empty() {
        return pairs;
    }

    out_ports
        .iter()
        .copied()
        .zip(in_ports.iter().copied())
        .collect()
}

#[must_use]
pub fn matcher_matches_node(matcher: &NodeMatcher, node: &NodeInfo) -> bool {
    if matcher.is_empty() {
        return false;
    }
    matches_opt(&matcher.node_name, node.node_name.as_deref())
        && matches_opt(&matcher.application_name, node.application_name.as_deref())
        && matches_opt(&matcher.media_class, node.media_class.as_deref())
}

#[must_use]
pub fn matcher_matches_endpoint(
    matcher: &NodeMatcher,
    node: Option<&NodeInfo>,
    port: Option<&PortInfo>,
) -> bool {
    if matcher.is_empty() {
        return false;
    }

    let needs_node = matcher.node_name.is_some()
        || matcher.application_name.is_some()
        || matcher.media_class.is_some();

    if needs_node {
        let Some(node) = node else {
            return false;
        };
        if !matches_opt(&matcher.node_name, node.node_name.as_deref())
            || !matches_opt(&matcher.application_name, node.application_name.as_deref())
            || !matches_opt(&matcher.media_class, node.media_class.as_deref())
        {
            return false;
        }
    }

    if matcher.port_name.is_some() {
        let Some(port) = port else {
            return false;
        };
        if !matches_opt(&matcher.port_name, port.port_name.as_deref()) {
            return false;
        }
    }

    true
}

fn matches_opt(pattern: &Option<String>, value: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(pat) => match value {
            Some(val) => wildcard_match_ci(pat, val),
            None => false,
        },
    }
}

#[must_use]
pub fn wildcard_match_ci(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let t: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
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
    use crate::config::{DspConfig, DspProfileRule};

    fn make_graph_with_nodes() -> AudioGraph {
        let mut graph = AudioGraph::new();
        graph.upsert_node(NodeInfo {
            id: 100,
            node_name: Some("Firefox".into()),
            node_description: None,
            application_name: Some("Firefox".into()),
            media_class: Some("Stream/Output/Audio".into()),
            node_latency: None,
            node_rate: None,
        });
        graph.upsert_port(PortInfo {
            id: 101,
            node_id: 100,
            port_name: Some("output_FL".into()),
            direction: PortDirection::Output,
            channel: Some("FL".into()),
        });
        graph.upsert_port(PortInfo {
            id: 102,
            node_id: 100,
            port_name: Some("output_FR".into()),
            direction: PortDirection::Output,
            channel: Some("FR".into()),
        });

        graph.upsert_node(NodeInfo {
            id: 200,
            node_name: Some("crimp_mix".into()),
            node_description: Some("Crimp Mix".into()),
            application_name: None,
            media_class: Some("Audio/Sink".into()),
            node_latency: None,
            node_rate: None,
        });
        graph.upsert_port(PortInfo {
            id: 201,
            node_id: 200,
            port_name: Some("playback_FL".into()),
            direction: PortDirection::Input,
            channel: Some("FL".into()),
        });
        graph.upsert_port(PortInfo {
            id: 202,
            node_id: 200,
            port_name: Some("playback_FR".into()),
            direction: PortDirection::Input,
            channel: Some("FR".into()),
        });

        graph.upsert_node(NodeInfo {
            id: 300,
            node_name: Some("alsa_output.pci-analog".into()),
            node_description: None,
            application_name: None,
            media_class: Some("Audio/Sink".into()),
            node_latency: None,
            node_rate: None,
        });
        graph.upsert_port(PortInfo {
            id: 301,
            node_id: 300,
            port_name: Some("playback_FL".into()),
            direction: PortDirection::Input,
            channel: Some("FL".into()),
        });

        graph
    }

    #[test]
    fn route_plans_stereo_links_and_drops_competing_wireplumber_link() {
        let mut graph = make_graph_with_nodes();
        graph.upsert_link(LinkInfo {
            id: 900,
            output_node_id: 100,
            output_port_id: 101,
            input_node_id: 300,
            input_port_id: 301,
        });

        let cfg = Config {
            route: vec![RouteRule {
                name: "Firefox to crimp_mix".into(),
                enabled: true,
                exclusive: true,
                source: NodeMatcher {
                    application_name: Some("Firefox".into()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    node_name: Some("crimp_mix".into()),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let engine = RuleEngine::new(cfg);
        let actions = engine.plan_routes(&graph);

        assert_eq!(actions.len(), 3);
        assert!(actions.contains(&RouteAction::DestroyCompetingLink {
            link_id: 900,
            rule_name: "Firefox to crimp_mix".into(),
        }));
    }

    #[test]
    fn deny_rule_prevents_route_create_loop() {
        let graph = make_graph_with_nodes();
        let cfg = Config {
            deny: vec![DenyRule {
                name: "Hard block".into(),
                enabled: true,
                source: NodeMatcher {
                    application_name: Some("Firefox".into()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    node_name: Some("crimp_mix".into()),
                    ..Default::default()
                },
            }],
            route: vec![RouteRule {
                name: "Conflicting".into(),
                enabled: true,
                exclusive: true,
                source: NodeMatcher {
                    application_name: Some("Firefox".into()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    node_name: Some("crimp_mix".into()),
                    ..Default::default()
                },
            }],
            ..Default::default()
        };

        let engine = RuleEngine::new(cfg);
        assert!(engine.plan_routes(&graph).is_empty());
    }

    #[test]
    fn dsp_profile_priority_and_override() {
        let mut graph = make_graph_with_nodes();
        graph.upsert_node(NodeInfo {
            id: 400,
            node_name: Some("REAPER".into()),
            node_description: None,
            application_name: Some("REAPER".into()),
            media_class: Some("Stream/Output/Audio".into()),
            node_latency: None,
            node_rate: None,
        });

        let cfg = Config {
            dsp: DspConfig {
                default_quantum: Some(1024),
                default_rate: Some(48000),
            },
            dsp_profile: vec![DspProfileRule {
                name: "Studio".into(),
                enabled: true,
                priority: 100,
                source: NodeMatcher {
                    application_name: Some("REAPER".into()),
                    ..Default::default()
                },
                quantum: Some(64),
                rate: Some(96000),
            }],
            ..Default::default()
        };

        let engine = RuleEngine::new(cfg);
        let dsp = engine.evaluate_dsp(&graph, None, None);
        assert_eq!((dsp.quantum, dsp.rate), (Some(64), Some(96000)));
    }

    #[test]
    fn resolves_custom_node_aliases_distinguishing_media_class() {
        let cfg = Config {
            alias: vec![
                crate::config::AliasRule {
                    matcher: NodeMatcher {
                        node_name: Some("java".into()),
                        media_class: Some("Stream/Output/Audio".into()),
                        ..Default::default()
                    },
                    display_name: "Minecraft (Zvuk)".into(),
                },
                crate::config::AliasRule {
                    matcher: NodeMatcher {
                        node_name: Some("java".into()),
                        media_class: Some("Stream/Input/Audio".into()),
                        ..Default::default()
                    },
                    display_name: "Minecraft (Mic)".into(),
                },
            ],
            ..Default::default()
        };
        let engine = RuleEngine::new(cfg);

        let mc_out = NodeInfo {
            id: 1,
            node_name: Some("java".into()),
            node_description: None,
            application_name: None,
            media_class: Some("Stream/Output/Audio".into()),
            node_latency: None,
            node_rate: None,
        };
        let mc_in = NodeInfo {
            id: 2,
            media_class: Some("Stream/Input/Audio".into()),
            ..mc_out.clone()
        };

        assert_eq!(
            engine.resolve_alias(&mc_out).as_deref(),
            Some("Minecraft (Zvuk)")
        );
        assert_eq!(
            engine.resolve_alias(&mc_in).as_deref(),
            Some("Minecraft (Mic)")
        );
    }
}
