use crate::client::CrimpClient;
use crate::theme;
use crimp_proto::config::{Config, VirtualSinkRule};
use crimp_proto::detect::{analyze_topology, apply_suggested_rules};
use crimp_proto::graph::{DeniedRecord, GraphSnapshot};
use crimp_proto::ipc::IpcRequest;
use crimp_proto::scenes::{SceneInfo, StarterSceneKind};
use egui::{Grid, ScrollArea};

pub struct RulesViewState {
    new_sink_name: String,
    new_sink_desc: String,
    new_sink_stereo: bool,
    new_scene_name: String,
}

impl Default for RulesViewState {
    fn default() -> Self {
        Self {
            new_sink_name: String::new(),
            new_sink_desc: String::new(),
            new_sink_stereo: true,
            new_scene_name: String::new(),
        }
    }
}

impl RulesViewState {
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: Option<&Config>,
        graph: Option<&GraphSnapshot>,
        denied_history: &[DeniedRecord],
        scenes: &[SceneInfo],
        active_scene_id: Option<&str>,
        client: &CrimpClient,
    ) {
        ScrollArea::vertical().show(ui, |ui| {
            // 0a. Scény / Presety (1-klik přepínání v UI i v systémové liště)
            if let Some(cfg) = config {
                theme::wide_card(ui, |ui| {
                    theme::section_label(ui, "Scény / Presety (1-klik přepínání i z tray ikony)");
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Ulož aktuální konfiguraci (DSP, routy, blokace, výchozí zařízení) jako pojmenovanou scénu. Objeví se i v horní liště a pravém kliku na tray ikonu.",
                        )
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_scene_name)
                                .desired_width(210.0)
                                .hint_text("Název scény (např. Stream / OBS)"),
                        );
                        if ui.button("Uložit aktuální jako scénu").clicked() {
                            let name = self.new_scene_name.trim().to_string();
                            if !name.is_empty() {
                                client.save_scene(name, cfg.clone());
                                self.new_scene_name.clear();
                            }
                        }
                        theme::spec_divider(ui);
                        for preset in [
                            StarterSceneKind::Gaming,
                            StarterSceneKind::Studio96k,
                            StarterSceneKind::AutoBalanced,
                        ] {
                            if ui
                                .button(format!("+ {}", preset.display_name()))
                                .on_hover_text(
                                    "Vytvoří startovací scénu z tvých aktuálních pravidel s přednastaveným DSP",
                                )
                                .clicked()
                            {
                                let next = preset.apply_to(cfg);
                                client.save_scene(preset.display_name().to_string(), next);
                            }
                        }
                    });

                    if !scenes.is_empty() {
                        ui.add_space(8.0);
                        for scene in scenes {
                            let is_active = active_scene_id == Some(scene.id.as_str());
                            ui.horizontal(|ui| {
                                if is_active {
                                    ui.colored_label(theme::ECONOMY_GREEN, "●");
                                }
                                ui.label(egui::RichText::new(&scene.name).strong());
                                let q_str = scene
                                    .quantum
                                    .map_or_else(|| "Auto".into(), |q| q.to_string());
                                let r_str = scene
                                    .rate
                                    .map_or_else(|| "Auto".into(), |r| format!("{r} Hz"));
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{q_str} @ {r_str}  ·  {} route, {} deny",
                                        scene.route_count, scene.deny_count
                                    ))
                                    .size(11.5)
                                    .color(theme::TEXT_MUTED),
                                );
                                if ui.selectable_label(is_active, "Aktivovat").clicked() {
                                    client.apply_scene(scene.id.clone());
                                }
                                if ui.small_button("Smazat").clicked() {
                                    client.delete_scene(scene.id.clone());
                                }
                            });
                        }
                    }
                });
                ui.add_space(10.0);
            }

            // 0b. Automatická detekce PipeWire hardwaru
            if let (Some(g), Some(cfg)) = (graph, config) {
                let topo = analyze_topology(g);
                theme::wide_card(ui, |ui| {
                    theme::section_label(ui, "Automatická detekce audio hardwaru");
                    ui.add_space(6.0);
                    Grid::new("auto_detect_grid").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        ui.label(egui::RichText::new("Doporučený výstup:").color(theme::TEXT_MUTED));
                        ui.label(
                            topo.preferred_sink
                                .as_ref()
                                .map_or_else(|| "-".to_string(), |d| format!("{} ({})", d.label, d.reason)),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new("Detekovaný mikrofon:").color(theme::TEXT_MUTED));
                        ui.label(
                            topo.preferred_mic
                                .as_ref()
                                .map_or_else(|| "-".to_string(), |d| format!("{} ({})", d.label, d.reason)),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new("HDMI monitory:").color(theme::TEXT_MUTED));
                        ui.label(format!("{} detekováno", topo.hdmi_monitors.len()));
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    if ui.button("Aplikovat doporučená pravidla (Auto-Detect)").clicked() {
                        let mut next = cfg.clone();
                        let added = apply_suggested_rules(&mut next, &topo);
                        if added > 0 {
                            client.send_command(IpcRequest::SaveConfig { config: next });
                        }
                    }
                });
                ui.add_space(10.0);
            }

            // 1. Nový virtuální sink
            theme::wide_card(ui, |ui| {
                theme::section_label(ui, "Přidat Virtuální Audio Sběrnici (Null Sink)");
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_sink_name)
                            .desired_width(170.0)
                            .hint_text("crimp_game_bus"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_sink_desc)
                            .desired_width(190.0)
                            .hint_text("Popis (např. Game Bus)"),
                    );
                    ui.checkbox(&mut self.new_sink_stereo, "Stereo (FL/FR)");
                    if ui.button("Vytvořit & Uložit").clicked()
                        && let Some(cfg) = config
                    {
                        let name = self.new_sink_name.trim().to_string();
                        if !name.is_empty() {
                            let mut next = cfg.clone();
                            next.virtual_sink.push(VirtualSinkRule {
                                name,
                                enabled: true,
                                description: (!self.new_sink_desc.trim().is_empty())
                                    .then(|| self.new_sink_desc.trim().to_string()),
                                media_class: "Audio/Sink".into(),
                                channels: if self.new_sink_stereo { 2 } else { 1 },
                                positions: if self.new_sink_stereo {
                                    vec!["FL".into(), "FR".into()]
                                } else {
                                    vec!["MONO".into()]
                                },
                            });
                            client.send_command(IpcRequest::SaveConfig { config: next });
                            self.new_sink_name.clear();
                            self.new_sink_desc.clear();
                        }
                    }
                });
            });

            ui.add_space(10.0);

            // 2. Pravidla z rules.toml
            if let Some(cfg) = config {
                theme::wide_card(ui, |ui| {
                    theme::section_label(ui, "Aktivní Pravidla v rules.toml");
                    ui.add_space(6.0);

                    if cfg.virtual_sink.is_empty()
                        && cfg.deny.is_empty()
                        && cfg.route.is_empty()
                        && cfg.dsp_profile.is_empty()
                    {
                        ui.label(
                            egui::RichText::new("Konfigurace neobsahuje žádná vlastní pravidla. Můžeš je přidat zde nebo kliknutím na kabely v Patchbayi.")
                                .color(theme::TEXT_MUTED),
                        );
                    }

                    let mut edited: Option<Config> = None;

                    for (idx, sink) in cfg.virtual_sink.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(theme::ECONOMY_GREEN, "VIRTUAL");
                            ui.label(&sink.name);
                            ui.label(
                                egui::RichText::new(format!("({} ch)", sink.channels))
                                    .color(theme::TEXT_MUTED),
                            );
                            if ui.small_button("Odebrat").clicked() {
                                let mut next = cfg.clone();
                                next.virtual_sink.remove(idx);
                                edited = Some(next);
                            }
                        });
                    }

                    for (idx, rule) in cfg.deny.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(theme::DANGER_RED, "DENY");
                            let mut on = rule.enabled;
                            if ui.checkbox(&mut on, &rule.name).changed() {
                                let mut next = cfg.clone();
                                next.deny[idx].enabled = on;
                                edited = Some(next);
                            }
                            if ui.small_button("Smazat").clicked() {
                                let mut next = cfg.clone();
                                next.deny.remove(idx);
                                edited = Some(next);
                            }
                        });
                    }

                    for (idx, rule) in cfg.route.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(theme::ACCENT_BLUE, "ROUTE");
                            let mut on = rule.enabled;
                            if ui.checkbox(&mut on, &rule.name).changed() {
                                let mut next = cfg.clone();
                                next.route[idx].enabled = on;
                                edited = Some(next);
                            }
                            if ui.small_button("Smazat").clicked() {
                                let mut next = cfg.clone();
                                next.route.remove(idx);
                                edited = Some(next);
                            }
                        });
                    }

                    for (idx, prof) in cfg.dsp_profile.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(theme::TEXT_PRIMARY, "DSP");
                            let mut on = prof.enabled;
                            if ui
                                .checkbox(
                                    &mut on,
                                    format!(
                                        "{} (q={:?}, r={:?})",
                                        prof.name, prof.quantum, prof.rate
                                    ),
                                )
                                .changed()
                            {
                                let mut next = cfg.clone();
                                next.dsp_profile[idx].enabled = on;
                                edited = Some(next);
                            }
                            if ui.small_button("Smazat").clicked() {
                                let mut next = cfg.clone();
                                next.dsp_profile.remove(idx);
                                edited = Some(next);
                            }
                        });
                    }

                    for (idx, alias) in cfg.alias.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(theme::HARDWARE_AMBER, "ALIAS");
                            ui.label(format!(
                                "{}  <-  {}",
                                alias.display_name,
                                alias
                                    .matcher
                                    .node_name
                                    .as_deref()
                                    .or(alias.matcher.application_name.as_deref())
                                    .unwrap_or("*")
                            ));
                            if ui.small_button("Smazat").clicked() {
                                let mut next = cfg.clone();
                                next.alias.remove(idx);
                                edited = Some(next);
                            }
                        });
                    }

                    if let Some(next) = edited {
                        client.send_command(IpcRequest::SaveConfig { config: next });
                    }
                });
                ui.add_space(10.0);
            }

            // 3. Historie zablokovaných WirePlumber auto-linků
            theme::wide_card(ui, |ui| {
                theme::section_label(ui, "Historie Zablokovaných Auto-Linků");
                ui.add_space(6.0);
                if denied_history.is_empty() {
                    ui.label(
                        egui::RichText::new("Zatím nebyly zachyceny žádné blokované linky.")
                            .color(theme::TEXT_MUTED),
                    );
                } else {
                    Grid::new("denied_table")
                        .num_columns(3)
                        .striped(true)
                        .spacing([14.0, 4.0])
                        .show(ui, |ui| {
                            for item in denied_history.iter().rev().take(40) {
                                ui.colored_label(theme::DANGER_RED, &item.rule_name);
                                ui.label(format!("link#{}", item.link_id));
                                ui.label(format!("{} -> {}", item.source_label, item.target_label));
                                ui.end_row();
                            }
                        });
                }
            });
        });
    }
}
