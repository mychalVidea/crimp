use crate::config::{Config, DenyRule, NodeMatcher};
use crate::graph::{GraphSnapshot, NodeSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Rodina linuxové distribuce pro doporučení balíčků a kontrolu kompatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistroFamily {
    ArchLike,
    FedoraLike,
    DebianLike,
    OpenSuse,
    Other,
}

impl DistroFamily {
    #[must_use]
    pub fn install_dev_deps_hint(self) -> &'static str {
        match self {
            Self::ArchLike => "sudo pacman -S pipewire wireplumber clang pkgconf",
            Self::FedoraLike => "sudo dnf install pipewire-devel clang pkgconf-pkg-config",
            Self::DebianLike => {
                "sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang pkg-config"
            }
            Self::OpenSuse => "sudo zypper install pipewire-devel clang pkg-config",
            Self::Other => "Nainstalujte vývojové hlavičky libpipewire-0.3, clang a pkg-config",
        }
    }
}

/// Diagnostika lokálního Linux prostředí bez nutnosti mít spuštěný Crimp daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEnvironment {
    pub os_id: String,
    pub os_pretty_name: String,
    pub family: DistroFamily,
    pub arch: String,
    pub display_session: String,
    pub runtime_dir: Option<String>,
    pub pipewire_socket_present: bool,
    pub libpipewire_present: bool,
}

impl HostEnvironment {
    /// Zjistí distribuci, zobrazovací server a dostupnost PipeWire runtime knihoven/socketů.
    #[must_use]
    pub fn probe() -> Self {
        let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
        parse_os_release_and_probe(&os_release)
    }

    #[must_use]
    pub fn is_compatible_for_binaries(&self) -> bool {
        self.arch == "x86_64" && self.libpipewire_present
    }
}

fn parse_os_release_and_probe(raw: &str) -> HostEnvironment {
    let mut os_id = String::from("linux");
    let mut id_like = String::new();
    let mut os_pretty_name = String::from("Linux");

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("ID=") {
            os_id = unquote(val).to_ascii_lowercase();
        } else if let Some(val) = trimmed.strip_prefix("ID_LIKE=") {
            id_like = unquote(val).to_ascii_lowercase();
        } else if let Some(val) = trimmed.strip_prefix("PRETTY_NAME=") {
            os_pretty_name = unquote(val);
        }
    }

    let family = classify_distro(&os_id, &id_like);
    let arch = std::env::consts::ARCH.to_string();

    let display_session = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland".to_string()
    } else if std::env::var_os("DISPLAY").is_some() {
        "x11".to_string()
    } else {
        std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "headless".to_string())
    };

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty());

    let pipewire_socket_present = runtime_dir
        .as_ref()
        .is_some_and(|dir| Path::new(dir).join("pipewire-0").exists());

    let libpipewire_present = detect_libpipewire();

    HostEnvironment {
        os_id,
        os_pretty_name,
        family,
        arch,
        display_session,
        runtime_dir,
        pipewire_socket_present,
        libpipewire_present,
    }
}

/// Klasifikuje distribuci včetně derivátů (`cachyos`, `ultramarine`, `nobara`, `endeavouros`, `pop`, ...).
#[must_use]
pub fn classify_distro(os_id: &str, id_like: &str) -> DistroFamily {
    let combined = format!("{os_id} {id_like}").to_ascii_lowercase();
    if combined.contains("cachyos")
        || combined.contains("arch")
        || combined.contains("manjaro")
        || combined.contains("endeavour")
    {
        DistroFamily::ArchLike
    } else if combined.contains("ultramarine")
        || combined.contains("fedora")
        || combined.contains("nobara")
        || combined.contains("bazzite")
        || combined.contains("rhel")
    {
        DistroFamily::FedoraLike
    } else if combined.contains("ubuntu")
        || combined.contains("debian")
        || combined.contains("mint")
        || combined.contains("pop")
    {
        DistroFamily::DebianLike
    } else if combined.contains("suse") {
        DistroFamily::OpenSuse
    } else {
        DistroFamily::Other
    }
}

fn detect_libpipewire() -> bool {
    let candidates = [
        "/usr/lib/libpipewire-0.3.so.0",
        "/usr/lib64/libpipewire-0.3.so.0",
        "/lib/x86_64-linux-gnu/libpipewire-0.3.so.0",
        "/usr/lib/x86_64-linux-gnu/libpipewire-0.3.so.0",
        "/lib/aarch64-linux-gnu/libpipewire-0.3.so.0",
        "/usr/lib/aarch64-linux-gnu/libpipewire-0.3.so.0",
    ];
    candidates.iter().any(|p| PathBuf::from(p).exists())
}

fn unquote(val: &str) -> String {
    val.trim().trim_matches('"').trim_matches('\'').to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedDevice {
    pub id: u32,
    pub label: String,
    pub node_name: String,
    pub reason: String,
}

/// Výsledek automatické analýzy aktuálního PipeWire grafu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedTopology {
    pub preferred_sink: Option<DetectedDevice>,
    pub preferred_mic: Option<DetectedDevice>,
    pub hdmi_monitors: Vec<DetectedDevice>,
    pub virtual_sinks: Vec<DetectedDevice>,
}

/// Analyzuje uzly v `GraphSnapshot` a rozpozná výchozí hardware vs HDMI monitory.
#[must_use]
pub fn analyze_topology(graph: &GraphSnapshot) -> DetectedTopology {
    let mut out = DetectedTopology::default();

    let mut sink_candidates: Vec<(i32, &NodeSnapshot, String)> = Vec::new();
    let mut mic_candidates: Vec<(i32, &NodeSnapshot, String)> = Vec::new();

    for node in &graph.nodes {
        let Some(name) = node.node_name.as_deref() else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        let lower_desc = node
            .node_description
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let class = node.media_class.as_deref().unwrap_or("");

        if node.is_virtual_crimp {
            out.virtual_sinks.push(DetectedDevice {
                id: node.id,
                label: node.label.clone(),
                node_name: name.to_string(),
                reason: "Virtuální Crimp sink".into(),
            });
            continue;
        }

        if class == "Audio/Sink" || class == "Audio/Duplex" {
            let is_hdmi = lower_name.contains("hdmi") || lower_desc.contains("hdmi");
            if is_hdmi {
                out.hdmi_monitors.push(DetectedDevice {
                    id: node.id,
                    label: node.label.clone(),
                    node_name: format!("{name}.monitor"),
                    reason: "HDMI/DisplayPort audio výstup (často ruší auto-routing)".into(),
                });
            }

            let mut score = 10;
            let mut reason = "Audio výstup".to_string();
            if lower_name.contains("usb") {
                score += 40;
                reason = "USB DAC / headset".into();
            } else if lower_name.contains("bluez") {
                score += 30;
                reason = "Bluetooth audio".into();
            } else if lower_name.contains("analog") {
                score += 25;
                reason = "Analogový výstup".into();
            } else if is_hdmi {
                score -= 20;
            }
            sink_candidates.push((score, node, reason));
        }

        if (class == "Audio/Source" || class == "Audio/Source/Virtual")
            && !lower_name.ends_with(".monitor")
        {
            let mut score = 10;
            let mut reason = "Mikrofon / vstup".to_string();
            if lower_name.contains("usb") {
                score += 40;
                reason = "USB mikrofon".into();
            } else if lower_name.contains("analog") {
                score += 25;
                reason = "Analogový mikrofon".into();
            }
            mic_candidates.push((score, node, reason));
        }
    }

    if let Some((_, best_sink, reason)) = sink_candidates
        .into_iter()
        .max_by_key(|(s, n, _)| (*s, std::cmp::Reverse(n.id)))
    {
        out.preferred_sink = Some(DetectedDevice {
            id: best_sink.id,
            label: best_sink.label.clone(),
            node_name: best_sink.node_name.clone().unwrap_or_default(),
            reason,
        });
    }

    if let Some((_, best_mic, reason)) = mic_candidates
        .into_iter()
        .max_by_key(|(s, n, _)| (*s, std::cmp::Reverse(n.id)))
    {
        out.preferred_mic = Some(DetectedDevice {
            id: best_mic.id,
            label: best_mic.label.clone(),
            node_name: best_mic.node_name.clone().unwrap_or_default(),
            reason,
        });
    }

    out
}

/// Vygeneruje bezpečná doporučená pravidla (např. blokaci HDMI `.monitor` pro hlasové aplikace)
/// a doplní je pouze tehdy, pokud ještě v `config` nejsou.
pub fn apply_suggested_rules(config: &mut Config, topo: &DetectedTopology) -> usize {
    let existing_names: HashSet<String> = config.deny.iter().map(|r| r.name.clone()).collect();
    let mut added = 0;

    if !topo.hdmi_monitors.is_empty() {
        let rule_name = "Auto: Zákaz HDMI monitoringu do hlasových aplikací (WebRTC)".to_string();
        if !existing_names.contains(&rule_name) {
            config.deny.push(DenyRule {
                name: rule_name,
                enabled: true,
                source: NodeMatcher {
                    node_name: Some("alsa_output.*hdmi*.monitor".to_string()),
                    ..Default::default()
                },
                target: NodeMatcher {
                    application_name: Some("WEBRTC VoiceEngine".to_string()),
                    ..Default::default()
                },
            });
            added += 1;
        }
    }

    if config.pw_settings.default_audio_sink.is_none()
        && let Some(sink) = &topo.preferred_sink
    {
        config.pw_settings.default_audio_sink = Some(sink.node_name.clone());
        added += 1;
    }

    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::PortInfo;

    #[test]
    fn classifies_cachyos_ultramarine_fedora_ubuntu() {
        assert_eq!(classify_distro("cachyos", "arch"), DistroFamily::ArchLike);
        assert_eq!(
            classify_distro("ultramarine", "fedora"),
            DistroFamily::FedoraLike
        );
        assert_eq!(classify_distro("fedora", ""), DistroFamily::FedoraLike);
        assert_eq!(
            classify_distro("ubuntu", "debian"),
            DistroFamily::DebianLike
        );
    }

    #[test]
    fn topology_prefers_usb_dac_over_hdmi_and_suggests_deny() {
        let graph = GraphSnapshot {
            nodes: vec![
                NodeSnapshot {
                    id: 1,
                    label: "GPU HDMI".into(),
                    node_name: Some("alsa_output.pci-0000_01_00.1.hdmi-stereo".into()),
                    node_description: Some("HDMI Audio".into()),
                    application_name: None,
                    media_class: Some("Audio/Sink".into()),
                    node_latency: None,
                    node_rate: None,
                    is_virtual_crimp: false,
                    output_ports: vec![],
                    input_ports: vec![],
                },
                NodeSnapshot {
                    id: 2,
                    label: "HyperX Cloud".into(),
                    node_name: Some("alsa_output.usb-Kingston_HyperX-00.analog-stereo".into()),
                    node_description: Some("USB Headset".into()),
                    application_name: None,
                    media_class: Some("Audio/Sink".into()),
                    node_latency: None,
                    node_rate: None,
                    is_virtual_crimp: false,
                    output_ports: vec![],
                    input_ports: vec![],
                },
                NodeSnapshot {
                    id: 3,
                    label: "USB Mic".into(),
                    node_name: Some("alsa_input.usb-Blue_Yeti-00.mono-fallback".into()),
                    node_description: None,
                    application_name: None,
                    media_class: Some("Audio/Source".into()),
                    node_latency: None,
                    node_rate: None,
                    is_virtual_crimp: false,
                    output_ports: Vec::<PortInfo>::new(),
                    input_ports: Vec::<PortInfo>::new(),
                },
            ],
            links: vec![],
        };

        let topo = analyze_topology(&graph);
        assert_eq!(
            topo.preferred_sink.as_ref().map(|d| d.id),
            Some(2),
            "USB DAC musí mít přednost před HDMI"
        );
        assert_eq!(topo.preferred_mic.as_ref().map(|d| d.id), Some(3));
        assert_eq!(topo.hdmi_monitors.len(), 1);

        let mut cfg = Config::default();
        let added = apply_suggested_rules(&mut cfg, &topo);
        assert_eq!(added, 2);
        assert_eq!(cfg.deny.len(), 1);
    }
}
