use crate::client::CrimpClient;
use crate::theme;
use crimp_proto::config::Config;
use crimp_proto::graph::{GraphSnapshot, PwRuntimeState};
use crimp_proto::ipc::{
    DaemonStatus, DefaultDeviceKind, DspOverrideChange, IpcRequest, MetadataTarget,
};
use egui::{Grid, ScrollArea};

pub struct SettingsViewState {
    min_quantum_input: String,
    max_quantum_input: String,
    quantum_limit_input: String,
    allowed_rates_input: String,
    initialized_from_runtime: bool,
}

impl Default for SettingsViewState {
    fn default() -> Self {
        Self {
            min_quantum_input: String::new(),
            max_quantum_input: String::new(),
            quantum_limit_input: String::new(),
            allowed_rates_input: String::new(),
            initialized_from_runtime: false,
        }
    }
}

impl SettingsViewState {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        status: Option<&DaemonStatus>,
        runtime: Option<&PwRuntimeState>,
        graph: Option<&GraphSnapshot>,
        config: Option<&Config>,
        client: &CrimpClient,
    ) {
        if let Some(rt) = runtime
            && !self.initialized_from_runtime
        {
            self.min_quantum_input = rt.clock_min_quantum.clone().unwrap_or_default();
            self.max_quantum_input = rt.clock_max_quantum.clone().unwrap_or_default();
            self.quantum_limit_input = rt.clock_quantum_limit.clone().unwrap_or_default();
            self.allowed_rates_input = rt.clock_allowed_rates.clone().unwrap_or_default();
            self.initialized_from_runtime = true;
        }

        ScrollArea::vertical().show(ui, |ui| {
            // 1. Přehled PipeWire Serveru a Latence
            theme::card_frame().show(ui, |ui| {
                theme::section_label(ui, "PipeWire Server & Živý Stav");
                ui.add_space(6.0);
                if let Some(rt) = runtime {
                    if let Some((q, r, ms)) = rt.effective_latency() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{ms:.2} ms"))
                                    .size(22.0)
                                    .color(theme::ECONOMY_GREEN)
                                    .strong(),
                            )
                            .on_hover_text(
                                "Zpoždění zpracování zvuku v jednom bufferu (quantum / frekvence). Do ~5 ms vhodné pro hry a DAW, 10–21 ms je úsporné pro filmy a běžný poslech.",
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "efektivní latence ({q} vzorků @ {r} Hz)"
                                ))
                                .color(theme::TEXT_MUTED),
                            );
                        });
                        ui.add_space(6.0);
                    }
                    Grid::new("pw_core_info")
                        .num_columns(4)
                        .spacing([24.0, 6.0])
                        .show(ui, |ui| {
                            kv(
                                ui,
                                "PipeWire Verze",
                                rt.core_version.as_deref().unwrap_or("-"),
                                "Verze běžícího PipeWire audio serveru v systému.",
                            );
                            kv(
                                ui,
                                "Core",
                                rt.core_name.as_deref().unwrap_or("-"),
                                "Název hlavní instance PipeWire serveru.",
                            );
                            ui.end_row();
                            kv(
                                ui,
                                "Aktivní Quantum",
                                rt.clock_force_quantum
                                    .as_deref()
                                    .or(rt.clock_quantum.as_deref())
                                    .unwrap_or("-"),
                                "Počet zvukových vzorků zpracovaných najednou. Menší = rychlejší odezva, větší = stabilnější při zátěži CPU.",
                            );
                            kv(
                                ui,
                                "Aktivní Sample Rate",
                                rt.clock_force_rate
                                    .as_deref()
                                    .or(rt.clock_rate.as_deref())
                                    .unwrap_or("-"),
                                "Vzorkovací frekvence grafu v Hz (48000 Hz je standard pro hry a videa, 96000 Hz pro studio/Hi-Res).",
                            );
                            ui.end_row();
                            kv(
                                ui,
                                "Crimp Profil",
                                status
                                    .and_then(|s| s.dsp_active_profile.as_deref())
                                    .unwrap_or("výchozí / auto"),
                                "Automatický DSP profil podle spuštěné aplikace (nebo ruční přepínač níže).",
                            );
                            kv(
                                ui,
                                "Výchozí Sink",
                                rt.default_audio_sink.as_deref().unwrap_or("-"),
                                "Výchozí výstupní zařízení (sluchátka/repro), kam míří nově spuštěné aplikace.",
                            );
                            ui.end_row();
                        });
                } else {
                    ui.label(
                        egui::RichText::new("PipeWire Runtime data nejsou k dispozici.")
                            .color(theme::TEXT_MUTED),
                    );
                }
            });

            ui.add_space(10.0);

            // 2. Okamžité přepínání DSP (clock.force-quantum & clock.force-rate)
            theme::card_frame().show(ui, |ui| {
                theme::section_label(ui, "Živé DSP přepínače (Buffer / Sample Rate)");
                ui.label(
                    egui::RichText::new(
                        "Najeď myší na libovolný název pro vysvětlení. Změny se projeví okamžitě bez restartu her ani programů.",
                    )
                    .size(11.5)
                    .color(theme::TEXT_MUTED),
                );
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Quantum (Buffer):").color(theme::TEXT_MUTED))
                        .on_hover_text(
                            "64–128 = nízká latence pro DAW a kompetitivní hry.\n256–512 = ideální zlatý střed.\n1024–2048 = minimální spotřeba CPU, nulové praskání.\nAuto = nechá PipeWire rozhodovat samo.",
                        );
                    for (q, hint) in [
                        (64u32, "Ultra nízká latence (~1.3 ms @ 48k)"),
                        (128, "Studio / nízká latence (~2.7 ms @ 48k)"),
                        (256, "Hraní / vyvážená latence (~5.3 ms @ 48k)"),
                        (512, "Běžné použití (~10.7 ms @ 48k)"),
                        (1024, "Úsporný buffer (~21.3 ms @ 48k)"),
                        (2048, "Maximální stabilita při extrémní zátěži"),
                    ] {
                        let active = status.and_then(|s| s.manual_quantum_override) == Some(q);
                        if ui
                            .selectable_label(active, format!("{q}"))
                            .on_hover_text(hint)
                            .clicked()
                        {
                            client.send_command(IpcRequest::SetDsp {
                                quantum: DspOverrideChange::Set(q),
                                rate: DspOverrideChange::Keep,
                            });
                        }
                    }
                    if ui
                        .selectable_label(
                            status.is_some_and(|s| s.manual_quantum_override.is_none()),
                            "Auto",
                        )
                        .on_hover_text("Vrátí řízení velikosti bufferu zpět PipeWire/aplikacím")
                        .clicked()
                    {
                        client.send_command(IpcRequest::SetDsp {
                            quantum: DspOverrideChange::Clear,
                            rate: DspOverrideChange::Keep,
                        });
                    }
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Sample Rate:").color(theme::TEXT_MUTED))
                        .on_hover_text(
                            "48000 Hz je výchozí standard pro Windows, Linux, hry i video.\n44100 Hz používají hudební CD/Spotify.\n96000+ Hz má smysl hlavně pro DACy a hudební produkci.",
                        );
                    for r in [44100u32, 48000, 88200, 96000, 192000] {
                        let active = status.and_then(|s| s.manual_rate_override) == Some(r);
                        if ui.selectable_label(active, format!("{r} Hz")).clicked() {
                            client.send_command(IpcRequest::SetDsp {
                                quantum: DspOverrideChange::Keep,
                                rate: DspOverrideChange::Set(r),
                            });
                        }
                    }
                    if ui
                        .selectable_label(
                            status.is_some_and(|s| s.manual_rate_override.is_none()),
                            "Auto",
                        )
                        .on_hover_text("Nechá PipeWire vybrat frekvenci automaticky podle zdroje zvuku")
                        .clicked()
                    {
                        client.send_command(IpcRequest::SetDsp {
                            quantum: DspOverrideChange::Keep,
                            rate: DspOverrideChange::Clear,
                        });
                    }
                });
            });

            ui.add_space(10.0);

            // 3. Pokročilá PipeWire Metadata nastavení (`settings`)
            theme::card_frame().show(ui, |ui| {
                theme::section_label(ui, "Pokročilé PipeWire Limity (Metadata 'settings')");
                ui.add_space(6.0);
                Grid::new("pw_limits_grid")
                    .num_columns(3)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        setting_row(
                            ui,
                            "clock.min-quantum",
                            &mut self.min_quantum_input,
                            "32",
                            "Nejnižší buffer, jaký si jakákoliv aplikace smí vyžádat. Pokud ti v hrách praská zvuk, nastav např. 128 nebo 256.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.max-quantum",
                            &mut self.max_quantum_input,
                            "2048",
                            "Nejvyšší buffer, na jaký smí PipeWire povyrůst při úspoře energie.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.quantum-limit",
                            &mut self.quantum_limit_input,
                            "8192",
                            "Tvrdý bezpečnostní strop velikosti bufferu v PipeWire.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.allowed-rates",
                            &mut self.allowed_rates_input,
                            "[ 44100 48000 96000 ]",
                            "Povolené vzorkovací frekvence v hranatých závorkách. Např. [ 48000 ] zamezí přepínání frekvencí při hraní/hudbě.",
                            MetadataTarget::Settings,
                            client,
                        );
                    });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let pow2 = runtime
                        .and_then(|rt| rt.clock_power_of_two.as_deref())
                        .unwrap_or("-");
                    ui.label(
                        egui::RichText::new(format!("clock.power-of-two-quantum: {pow2}"))
                            .color(theme::TEXT_MUTED),
                    )
                    .on_hover_text(
                        "Pokud je true, velikost bufferu musí být vždy mocnina dvojky (64, 128, 256, 512...). Většina hardwaru vyžaduje true.",
                    );
                    if ui.button("true").clicked() {
                        client.send_command(IpcRequest::SetPwSetting {
                            target: MetadataTarget::Settings,
                            key: "clock.power-of-two-quantum".into(),
                            value: Some("true".into()),
                        });
                    }
                    if ui.button("false").clicked() {
                        client.send_command(IpcRequest::SetPwSetting {
                            target: MetadataTarget::Settings,
                            key: "clock.power-of-two-quantum".into(),
                            value: Some("false".into()),
                        });
                    }
                    if let Some(cfg) = config
                        && ui
                            .button("Uložit limity natrvalo do rules.toml")
                            .on_hover_text("Zapíše min/max/limit quantum do konfigurace, takže přežijí i restart PC")
                            .clicked()
                    {
                        let mut next = cfg.clone();
                        next.pw_settings.min_quantum = self.min_quantum_input.trim().parse().ok();
                        next.pw_settings.max_quantum = self.max_quantum_input.trim().parse().ok();
                        next.pw_settings.quantum_limit =
                            self.quantum_limit_input.trim().parse().ok();
                        client.send_command(IpcRequest::SaveConfig { config: next });
                    }
                });
            });

            ui.add_space(10.0);

            // 4. Výchozí audio zařízení (`default`)
            if let Some(graph) = graph {
                theme::card_frame().show(ui, |ui| {
                    theme::section_label(ui, "Výchozí Audio Zařízení ('default')");
                    ui.add_space(6.0);
                    let cur_sink = runtime.and_then(|r| r.default_audio_sink.as_deref());
                    let cur_src = runtime.and_then(|r| r.default_audio_source.as_deref());

                    ui.label(egui::RichText::new("Výstupy (Sluchátka / Repro):").color(theme::TEXT_MUTED))
                        .on_hover_text("Vyber výchozí zvukový výstup pro celý systém");
                    ui.horizontal_wrapped(|ui| {
                        for node in graph
                            .nodes
                            .iter()
                            .filter(|n| n.media_class.as_deref().is_some_and(|c| c.contains("Sink")))
                        {
                            if let Some(name) = &node.node_name {
                                let is_cur = cur_sink == Some(name.as_str());
                                if ui
                                    .selectable_label(is_cur, &node.label)
                                    .on_hover_text(format!("PipeWire ID: {name}"))
                                    .clicked()
                                {
                                    client.send_command(IpcRequest::SetDefaultDevice {
                                        kind: DefaultDeviceKind::Sink,
                                        node_name: Some(name.clone()),
                                    });
                                }
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Vstupy / Mikrofony:").color(theme::TEXT_MUTED))
                        .on_hover_text("Vyber výchozí mikrofon pro prohlížeč, hry a Discord");
                    ui.horizontal_wrapped(|ui| {
                        for node in graph.nodes.iter().filter(|n| {
                            n.media_class
                                .as_deref()
                                .is_some_and(|c| c.contains("Source"))
                        }) {
                            if let Some(name) = &node.node_name {
                                let is_cur = cur_src == Some(name.as_str());
                                if ui
                                    .selectable_label(is_cur, &node.label)
                                    .on_hover_text(format!("PipeWire ID: {name}"))
                                    .clicked()
                                {
                                    client.send_command(IpcRequest::SetDefaultDevice {
                                        kind: DefaultDeviceKind::Source,
                                        node_name: Some(name.clone()),
                                    });
                                }
                            }
                        }
                    });
                });
                ui.add_space(10.0);
            }

            // 5. Kompletní inspektor všech surových PipeWire vlastností
            if let Some(rt) = runtime {
                theme::card_frame().show(ui, |ui| {
                    theme::section_label(ui, "Kompletní PipeWire Metadata & Core Properties");
                    ui.add_space(6.0);
                    Grid::new("raw_pw_table")
                        .num_columns(3)
                        .striped(true)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            for (k, v) in &rt.raw_settings_metadata {
                                ui.label(
                                    egui::RichText::new("settings")
                                        .color(theme::ACCENT_BLUE)
                                        .size(11.0),
                                );
                                ui.label(egui::RichText::new(k).monospace());
                                ui.label(
                                    egui::RichText::new(v)
                                        .monospace()
                                        .color(theme::TEXT_MUTED),
                                );
                                ui.end_row();
                            }
                            for (k, v) in &rt.raw_default_metadata {
                                ui.label(
                                    egui::RichText::new("default")
                                        .color(theme::ECONOMY_GREEN)
                                        .size(11.0),
                                );
                                ui.label(egui::RichText::new(k).monospace());
                                ui.label(
                                    egui::RichText::new(v)
                                        .monospace()
                                        .color(theme::TEXT_MUTED),
                                );
                                ui.end_row();
                            }
                            for (k, v) in &rt.raw_core_props {
                                ui.label(
                                    egui::RichText::new("core")
                                        .color(theme::TEXT_MUTED)
                                        .size(11.0),
                                );
                                ui.label(egui::RichText::new(k).monospace());
                                ui.label(
                                    egui::RichText::new(v)
                                        .monospace()
                                        .color(theme::TEXT_MUTED),
                                );
                                ui.end_row();
                            }
                        });
                });
            }
        });
    }
}

fn kv(ui: &mut egui::Ui, label: &str, value: &str, tooltip: &str) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(theme::TEXT_MUTED),
        )
        .on_hover_text(tooltip);
        ui.label(egui::RichText::new(value).strong())
            .on_hover_text(tooltip);
    });
}

fn setting_row(
    ui: &mut egui::Ui,
    key: &str,
    buf: &mut String,
    hint: &str,
    tooltip: &str,
    target: MetadataTarget,
    client: &CrimpClient,
) {
    ui.label(egui::RichText::new(key).monospace())
        .on_hover_text(tooltip);
    ui.add(
        egui::TextEdit::singleline(buf)
            .desired_width(180.0)
            .hint_text(hint),
    )
    .on_hover_text(tooltip);
    ui.horizontal(|ui| {
        if ui.button("Použít").clicked() {
            let trimmed = buf.trim();
            let val = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            client.send_command(IpcRequest::SetPwSetting {
                target,
                key: key.to_string(),
                value: val,
            });
        }
        if ui.button("Auto").clicked() {
            buf.clear();
            client.send_command(IpcRequest::SetPwSetting {
                target,
                key: key.to_string(),
                value: None,
            });
        }
    });
    ui.end_row();
}
