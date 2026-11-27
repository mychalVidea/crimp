use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Provozní stav PipeWire uzlu dle `pw-top` (`R` = běží zvuk, `I` = klid, `S` = uspaný).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeActivityState {
    Running,
    Idle,
    Suspended,
    Creating,
    Error,
    #[default]
    Unknown,
}

impl NodeActivityState {
    #[must_use]
    pub fn from_pw_top_token(token: &str) -> Self {
        match token.trim() {
            "R" => Self::Running,
            "I" => Self::Idle,
            "S" => Self::Suspended,
            "C" => Self::Creating,
            "E" => Self::Error,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn label_cs(self) -> &'static str {
        match self {
            Self::Running => "Aktivní (teče signál)",
            Self::Idle => "Připraven (ticho)",
            Self::Suspended => "Uspaný",
            Self::Creating => "Startuje",
            Self::Error => "Chyba",
            Self::Unknown => "Neznámý",
        }
    }
}

/// Živé telemetrické metriky jednoho PipeWire uzlu z `pw-top -b`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub id: u32,
    pub state: NodeActivityState,
    pub quantum: u32,
    pub rate: u32,
    /// Poměr vytížení DSP (`B/Q`), např. `0.04` = 4 % okna bufferu.
    pub busy_ratio: f32,
    /// Počet výpadků / xruns hlášených PipeWire profilerem (`ERR`).
    pub err_count: u64,
    pub name: String,
}

/// Souhrnný živý stav zdraví PipeWire grafu (xruns + aktivita uzlů).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioHealthSnapshot {
    pub available: bool,
    pub total_xruns: u64,
    pub active_nodes: usize,
    pub per_node: BTreeMap<u32, NodeMetrics>,
}

/// Živá hlasitost a stav ztlumení uzlu z WirePlumberu (`wpctl`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NodeVolumeState {
    /// Lineární hlasitost (1.0 = 100 %, max typicky 1.5 = 150 %).
    pub volume: f32,
    pub muted: bool,
}

impl Default for NodeVolumeState {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
        }
    }
}

/// Zparsuje jednu dávku výstupu z `pw-top -b` na `AudioHealthSnapshot`.
#[must_use]
pub fn parse_pw_top_batch(raw: &str) -> AudioHealthSnapshot {
    let mut snap = AudioHealthSnapshot {
        available: false,
        ..Default::default()
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 9 {
            continue;
        }
        if cols[0] == "S" && cols[1] == "ID" {
            snap.available = true;
            continue;
        }

        let Ok(id) = cols[1].parse::<u32>() else {
            continue;
        };
        let state = NodeActivityState::from_pw_top_token(cols[0]);
        let quantum = cols[2].parse::<u32>().unwrap_or(0);
        let rate = cols[3].parse::<u32>().unwrap_or(0);
        let busy_ratio = parse_pw_top_float(cols[7]).unwrap_or(0.0).max(0.0);
        let err_count = cols[8].parse::<u64>().unwrap_or(0);
        let name = if cols.len() > 10 {
            cols[10..].join(" ")
        } else if cols.len() > 9 {
            cols[9].to_string()
        } else {
            String::new()
        };

        snap.available = true;
        if state == NodeActivityState::Running {
            snap.active_nodes += 1;
        }
        snap.total_xruns = snap.total_xruns.saturating_add(err_count);
        snap.per_node.insert(
            id,
            NodeMetrics {
                id,
                state,
                quantum,
                rate,
                busy_ratio,
                err_count,
                name,
            },
        );
    }

    snap
}

fn parse_pw_top_float(raw: &str) -> Option<f32> {
    let clean = raw.trim().replace(',', ".");
    if clean.is_empty() || clean.starts_with('-') {
        return None;
    }
    clean.parse::<f32>().ok()
}

/// Zparsuje hlasitosti všech uzlů z hromadného výstupu `wpctl status`.
/// Hledá řádky s ID uzlu a sekcí `[vol: 0.85]` / `[vol: 0.85 MUTED]`.
#[must_use]
pub fn parse_wpctl_status_volumes(raw: &str) -> BTreeMap<u32, NodeVolumeState> {
    let mut out = BTreeMap::new();

    for line in raw.lines() {
        let Some(vol_start) = line.find("[vol:") else {
            continue;
        };
        let rest = &line[vol_start + 5..];
        let Some(vol_end) = rest.find(']') else {
            continue;
        };
        let vol_body = rest[..vol_end].trim();
        let mut parts = vol_body.split_whitespace();
        let Some(num_tok) = parts.next() else {
            continue;
        };
        let Ok(volume) = num_tok.replace(',', ".").parse::<f32>() else {
            continue;
        };
        let muted = parts.any(|p| p.eq_ignore_ascii_case("MUTED"));

        if let Some(id) = extract_wpctl_node_id(&line[..vol_start]) {
            out.insert(
                id,
                NodeVolumeState {
                    volume: volume.clamp(0.0, 1.5),
                    muted,
                },
            );
        }
    }

    out
}

/// Zparsuje výstup příkazu `wpctl get-volume <ID>` (např. `Volume: 0.85 [MUTED]`).
#[must_use]
pub fn parse_wpctl_get_volume(raw: &str) -> Option<NodeVolumeState> {
    let line = raw.lines().next()?.trim();
    let rest = line.split_once(':')?.1.trim();
    let mut parts = rest.split_whitespace();
    let num_tok = parts.next()?;
    let volume = num_tok.replace(',', ".").parse::<f32>().ok()?;
    let muted = rest.to_ascii_uppercase().contains("MUTED");
    Some(NodeVolumeState {
        volume: volume.clamp(0.0, 1.5),
        muted,
    })
}

fn extract_wpctl_node_id(prefix: &str) -> Option<u32> {
    // Typický řádek: ` │  *   54. Ryzen HD Audio ...` nebo ` │     108. java ...`
    let dot_idx = prefix.find('.')?;
    let before_dot = prefix[..dot_idx].trim();
    let digits = before_dot
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|s| !s.is_empty())?;
    digits.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pw_top_batch_with_xruns_and_activity() {
        let sample = "\
S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME
S   29      0      0    ---     ---   ---   ---     0                  Dummy-Driver
R   54    512  96000  42.1us  18.5us  0.01  0.04    3    S32LE 2 96000 alsa_output.pci-0000.analog-stereo
R   92      0      0  11.0us   9.2us  0.00  0.02    1    F32LE 2 48000 OBS
I  108      0      0    ---     ---   ---   ---     0    F32LE 2 48000 cs2
";

        let snap = parse_pw_top_batch(sample);
        assert!(snap.available);
        assert_eq!(snap.active_nodes, 2);
        assert_eq!(snap.total_xruns, 4);

        let sink = snap.per_node.get(&54).expect("sink metrics");
        assert_eq!(sink.state, NodeActivityState::Running);
        assert_eq!(sink.quantum, 512);
        assert_eq!(sink.rate, 96000);
        assert_eq!(sink.err_count, 3);
        assert!((sink.busy_ratio - 0.04).abs() < 0.001);

        let cs2 = snap.per_node.get(&108).expect("cs2 metrics");
        assert_eq!(cs2.state, NodeActivityState::Idle);
    }

    #[test]
    fn parses_wpctl_status_and_get_volume() {
        let status = "\
Audio
 ├─ Sinks:
 │  *   54. Ryzen HD Audio Controller Analogové stereo [vol: 0.72]
 │      60. HDMI Audio                                  [vol: 1.00 MUTED]
 ├─ Sources:
 │  *   52. HyperX DuoCast Digitální stereo            [vol: 0.95]
 └─ Streams:
        108. java                                       [vol: 0.50]
";

        let vols = parse_wpctl_status_volumes(status);
        assert_eq!(vols.len(), 4);
        assert!((vols[&54].volume - 0.72).abs() < 0.001);
        assert!(!vols[&54].muted);
        assert!(vols[&60].muted);
        assert!((vols[&52].volume - 0.95).abs() < 0.001);
        assert!((vols[&108].volume - 0.50).abs() < 0.001);

        let single = parse_wpctl_get_volume("Volume: 0.64 [MUTED]\n").expect("single vol");
        assert!((single.volume - 0.64).abs() < 0.001);
        assert!(single.muted);
    }
}
