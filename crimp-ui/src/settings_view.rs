use crate::autostart::AutostartState;
use crate::client::CrimpClient;
use crate::theme;
use crimp_proto::config::Config;
use crimp_proto::graph::{GraphSnapshot, PwRuntimeState};
use crimp_proto::ipc::{
    DaemonStatus, DefaultDeviceKind, DspOverrideChange, IpcRequest, MetadataTarget,
};
use egui::{CornerRadius, Grid, ScrollArea, Stroke};

#[derive(Default)]
pub struct SettingsViewState {
    min_quantum_input: String,
    max_quantum_input: String,
    quantum_limit_input: String,
    allowed_rates_input: String,
    min_quantum_dirty: bool,
    max_quantum_dirty: bool,
    quantum_limit_dirty: bool,
    allowed_rates_dirty: bool,
}

impl SettingsViewState {
    pub fn reset_dirty_inputs(&mut self) {
        self.min_quantum_dirty = false;
        self.max_quantum_dirty = false;
        self.quantum_limit_dirty = false;
        self.allowed_rates_dirty = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        status: Option<&DaemonStatus>,
        runtime: Option<&PwRuntimeState>,
        graph: Option<&GraphSnapshot>,
        config: Option<&Config>,
        autostart: &AutostartState,
        client: &CrimpClient,
    ) -> bool {
        let mut reload_all_clicked = false;

        if config.is_some() || runtime.is_some() {
            let desired_min = config
                .and_then(|c| c.pw_settings.min_quantum.map(|v| v.to_string()))
                .or_else(|| runtime.and_then(|r| r.clock_min_quantum.clone()));
            let desired_max = config
                .and_then(|c| c.pw_settings.max_quantum.map(|v| v.to_string()))
                .or_else(|| runtime.and_then(|r| r.clock_max_quantum.clone()));
            let desired_limit = config
                .and_then(|c| c.pw_settings.quantum_limit.map(|v| v.to_string()))
                .or_else(|| runtime.and_then(|r| r.clock_quantum_limit.clone()));
            let desired_rates = config
                .and_then(|c| {
                    c.pw_settings
                        .allowed_rates
                        .as_deref()
                        .map(crimp_proto::config::format_allowed_rates)
                })
                .or_else(|| runtime.and_then(|r| r.clock_allowed_rates.clone()));

            sync_input_field(
                &mut self.min_quantum_input,
                self.min_quantum_dirty,
                desired_min,
            );
            sync_input_field(
                &mut self.max_quantum_input,
                self.max_quantum_dirty,
                desired_max,
            );
            sync_input_field(
                &mut self.quantum_limit_input,
                self.quantum_limit_dirty,
                desired_limit,
            );
            sync_input_field(
                &mut self.allowed_rates_input,
                self.allowed_rates_dirty,
                desired_rates,
            );
        }

        ScrollArea::vertical().show(ui, |ui| {
            let full_w = ui.available_width().max(540.0);

            // 0. Zřetelná prioritní karta: Start se systémem & pravidla
            let auto_on = autostart.is_fully_enabled();
            let accent_border = if auto_on {
                egui::Color32::from_rgba_unmultiplied(33, 222, 0, 95)
            } else {
                egui::Color32::from_rgba_unmultiplied(10, 103, 229, 130)
            };
            theme::wide_accent_card(ui, accent_border, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width((full_w - 300.0).max(260.0));
                        ui.horizontal(|ui| {
                            theme::section_label(ui, "Start se systémem & Pravidla");
                            ui.add_space(6.0);
                            let (badge_txt, badge_col) = if auto_on {
                                ("● AKTIVNÍ PO PŘIHLÁŠENÍ", theme::ECONOMY_GREEN)
                            } else {
                                ("○ VYPNUTO", theme::TEXT_SUBTLE)
                            };
                            ui.label(
                                egui::RichText::new(badge_txt)
                                    .size(11.5)
                                    .strong()
                                    .color(badge_col),
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Spouštět Crimp automaticky při startu PC")
                                .size(16.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(
                                "Po přihlášení se na pozadí spustí daemon (ihned načte pravidla, blokace, aliasy a virtuální sběrnice) a v liště zůstane jen lehká tray ikona. Hlavní GUI okno se samo neotevře.",
                            )
                            .size(13.0)
                            .color(theme::TEXT_MUTED),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            status_dot(
                                ui,
                                autostart.daemon_enabled,
                                "Služba daemonu (crimp.service)",
                            );
                            ui.add_space(10.0);
                            theme::spec_divider(ui);
                            ui.add_space(10.0);
                            status_dot(
                                ui,
                                autostart.tray_enabled,
                                "Tray ikona (~/.config/autostart)",
                            );
                        });
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.vertical(|ui| {
                            let toggle_btn = if auto_on {
                                egui::Button::new(
                                    egui::RichText::new("✓ Start při přihlášení: ZAPNUTO")
                                        .size(13.5)
                                        .strong()
                                        .color(theme::ECONOMY_GREEN),
                                )
                                .fill(egui::Color32::from_rgb(16, 42, 20))
                                .stroke(Stroke::new(1.2, theme::ECONOMY_GREEN))
                                .corner_radius(CornerRadius::same(8))
                                .min_size(egui::vec2(236.0, 38.0))
                            } else {
                                egui::Button::new(
                                    egui::RichText::new("Zapnout start při přihlášení")
                                        .size(13.5)
                                        .strong()
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(theme::ACCENT_BLUE)
                                .stroke(Stroke::NONE)
                                .corner_radius(CornerRadius::same(8))
                                .min_size(egui::vec2(236.0, 38.0))
                            };
                            if ui
                                .add(toggle_btn)
                                .on_hover_text(
                                    "Zapne nebo vypne `crimp.service` + `~/.config/autostart/crimp-tray.desktop`",
                                )
                                .clicked()
                            {
                                client.set_autostart(!auto_on);
                            }

                            ui.add_space(4.0);
                            if ui
                                .add(
                                    egui::Button::new("Načíst pravidla a rozložení")
                                        .min_size(egui::vec2(236.0, 30.0))
                                        .corner_radius(CornerRadius::same(7)),
                                )
                                .on_hover_text(
                                    "Znovu načte `rules.toml` v daemonu i uložené pozice uzlů z `ui-layout.json`",
                                )
                                .clicked()
                            {
                                reload_all_clicked = true;
                            }
                        });
                    });
                });
            });

            ui.add_space(12.0);

            // 1. Horní bento řada: Živý stav + Rychlé DSP přepínače vedle sebe (při užším okně pod sebou)
            if full_w >= 860.0 {
                ui.columns(2, |cols| {
                    draw_live_status_card(&mut cols[0], status, runtime, config);
                    draw_dsp_card(&mut cols[1], status, config, client);
                });
            } else {
                draw_live_status_card(ui, status, runtime, config);
                ui.add_space(12.0);
                draw_dsp_card(ui, status, config, client);
            }

            ui.add_space(12.0);

            // 2. Výchozí audio zařízení (`default`) - výstupy i vstupy pohodlně vedle sebe
            if let Some(graph) = graph {
                theme::wide_card(ui, |ui| {
                    theme::section_label(ui, "Výchozí Audio Zařízení ('default')");
                    ui.add_space(8.0);
                    let saved_sink = config.and_then(|c| c.pw_settings.default_audio_sink.as_deref());
                    let saved_src =
                        config.and_then(|c| c.pw_settings.default_audio_source.as_deref());
                    let cur_sink =
                        saved_sink.or_else(|| runtime.and_then(|r| r.default_audio_sink.as_deref()));
                    let cur_src = saved_src
                        .or_else(|| runtime.and_then(|r| r.default_audio_source.as_deref()));

                    let render_devices = |ui: &mut egui::Ui| {
                        let draw_sinks = |ui: &mut egui::Ui| {
                            ui.label(
                                egui::RichText::new("Výstupy (Sluchátka / Repro)")
                                    .size(12.5)
                                    .color(theme::TEXT_MUTED),
                            )
                            .on_hover_text("Vyber výchozí zvukový výstup pro celý systém");
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                for node in graph.nodes.iter().filter(|n| {
                                    n.media_class
                                        .as_deref()
                                        .is_some_and(|c| c.starts_with("Audio/Sink") || c == "Audio/Duplex")
                                }) {
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
                                if saved_sink.is_some()
                                    && ui
                                        .button("Auto")
                                        .on_hover_text(
                                            "Odstraní připnutý výstup z rules.toml a nechá volbu na WirePlumberu",
                                        )
                                        .clicked()
                                {
                                    client.send_command(IpcRequest::SetDefaultDevice {
                                        kind: DefaultDeviceKind::Sink,
                                        node_name: None,
                                    });
                                }
                            });
                        };

                        let draw_sources = |ui: &mut egui::Ui| {
                            ui.label(
                                egui::RichText::new("Vstupy / Mikrofony")
                                    .size(12.5)
                                    .color(theme::TEXT_MUTED),
                            )
                            .on_hover_text("Vyber výchozí mikrofon pro prohlížeč, hry a Discord");
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                for node in graph.nodes.iter().filter(|n| {
                                    n.media_class.as_deref().is_some_and(|c| {
                                        c.starts_with("Audio/Source") || c == "Audio/Duplex"
                                    })
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
                                if saved_src.is_some()
                                    && ui
                                        .button("Auto")
                                        .on_hover_text(
                                            "Odstraní připnutý mikrofon z rules.toml a nechá volbu na WirePlumberu",
                                        )
                                        .clicked()
                                {
                                    client.send_command(IpcRequest::SetDefaultDevice {
                                        kind: DefaultDeviceKind::Source,
                                        node_name: None,
                                    });
                                }
                            });
                        };

                        if full_w >= 860.0 {
                            ui.columns(2, |cols| {
                                cols[0].vertical(draw_sinks);
                                cols[1].vertical(draw_sources);
                            });
                        } else {
                            draw_sinks(ui);
                            ui.add_space(8.0);
                            draw_sources(ui);
                        }
                    };
                    render_devices(ui);
                });
                ui.add_space(12.0);
            }

            // 3. Pokročilá PipeWire Metadata nastavení (`settings`) přes celou šířku
            theme::wide_card(ui, |ui| {
                theme::section_label(ui, "Pokročilé PipeWire Limity (Metadata 'settings')");
                ui.add_space(8.0);
                Grid::new("pw_limits_grid")
                    .num_columns(3)
                    .spacing([18.0, 10.0])
                    .show(ui, |ui| {
                        setting_row(
                            ui,
                            "clock.min-quantum",
                            &mut self.min_quantum_input,
                            &mut self.min_quantum_dirty,
                            "32",
                            "Nejnižší buffer, jaký si jakákoliv aplikace smí vyžádat. Pokud ti v hrách praská zvuk, nastav např. 128 nebo 256.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.max-quantum",
                            &mut self.max_quantum_input,
                            &mut self.max_quantum_dirty,
                            "2048",
                            "Nejvyšší buffer, na jaký smí PipeWire povyrůst při úspoře energie.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.quantum-limit",
                            &mut self.quantum_limit_input,
                            &mut self.quantum_limit_dirty,
                            "8192",
                            "Tvrdý bezpečnostní strop velikosti bufferu v PipeWire.",
                            MetadataTarget::Settings,
                            client,
                        );
                        setting_row(
                            ui,
                            "clock.allowed-rates",
                            &mut self.allowed_rates_input,
                            &mut self.allowed_rates_dirty,
                            "[ 44100 48000 96000 ]",
                            "Povolené vzorkovací frekvence v hranatých závorkách. Např. [ 48000 ] zamezí přepínání frekvencí při hraní/hudbě.",
                            MetadataTarget::Settings,
                            client,
                        );
                    });

                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    let saved_pow2 = config.and_then(|c| c.pw_settings.power_of_two_quantum);
                    let live_pow2 = runtime
                        .and_then(|rt| rt.clock_power_of_two.as_deref())
                        .and_then(|v| match v.trim() {
                            "true" | "1" => Some(true),
                            "false" | "0" => Some(false),
                            _ => None,
                        });
                    let effective_pow2 = saved_pow2.or(live_pow2);
                    let pow2_label = match effective_pow2 {
                        Some(true) => "true",
                        Some(false) => "false",
                        None => "auto",
                    };
                    ui.label(
                        egui::RichText::new(format!("clock.power-of-two-quantum: {pow2_label}"))
                            .color(theme::TEXT_MUTED),
                    )
                    .on_hover_text(
                        "Pokud je true, velikost bufferu musí být vždy mocnina dvojky (64, 128, 256, 512...). Většina hardwaru vyžaduje true.",
                    );
                    if ui
                        .selectable_label(effective_pow2 == Some(true), "true")
                        .clicked()
                    {
                        client.send_command(IpcRequest::SetPwSetting {
                            target: MetadataTarget::Settings,
                            key: "clock.power-of-two-quantum".into(),
                            value: Some("true".into()),
                        });
                    }
                    if ui
                        .selectable_label(effective_pow2 == Some(false), "false")
                        .clicked()
                    {
                        client.send_command(IpcRequest::SetPwSetting {
                            target: MetadataTarget::Settings,
                            key: "clock.power-of-two-quantum".into(),
                            value: Some("false".into()),
                        });
                    }
                    if ui
                        .selectable_label(saved_pow2.is_none(), "Auto")
                        .on_hover_text("Vrátí power-of-two-quantum na výchozí chování PipeWire")
                        .clicked()
                    {
                        client.send_command(IpcRequest::SetPwSetting {
                            target: MetadataTarget::Settings,
                            key: "clock.power-of-two-quantum".into(),
                            value: None,
                        });
                    }
                    if ui
                        .button("Uložit limity natrvalo do rules.toml")
                        .on_hover_text(
                            "Zapíše min/max/limit quantum a allowed-rates do konfigurace, takže přežijí i restart PC",
                        )
                        .clicked()
                    {
                        match (
                            parse_opt_u32_limit("clock.min-quantum", &self.min_quantum_input),
                            parse_opt_u32_limit("clock.max-quantum", &self.max_quantum_input),
                            parse_opt_u32_limit("clock.quantum-limit", &self.quantum_limit_input),
                            parse_opt_allowed_rates(&self.allowed_rates_input),
                        ) {
                            (Ok(min_q), Ok(max_q), Ok(lim_q), Ok(rates)) => {
                                if let (Some(min_v), Some(max_v)) = (min_q, max_q)
                                    && min_v > max_v
                                {
                                    client.push_feedback(
                                        true,
                                        format!(
                                            "clock.min-quantum ({min_v}) nesmí být větší než clock.max-quantum ({max_v})"
                                        ),
                                    );
                                } else {
                                    client.send_command(IpcRequest::SavePwLimits {
                                        min_quantum: min_q,
                                        max_quantum: max_q,
                                        quantum_limit: lim_q,
                                        allowed_rates: rates,
                                    });
                                    self.reset_dirty_inputs();
                                }
                            }
                            (Err(e), _, _, _)
                            | (_, Err(e), _, _)
                            | (_, _, Err(e), _)
                            | (_, _, _, Err(e)) => {
                                client.push_feedback(true, e);
                            }
                        }
                    }
                });
            });

            ui.add_space(12.0);

            // 4. Kompletní inspektor surových PipeWire vlastností (rozbalovací, nerozbíjí rozvržení)
            if let Some(rt) = runtime {
                theme::wide_card(ui, |ui| {
                    egui::CollapsingHeader::new(
                        egui::RichText::new("DETAILNÍ PIPEWIRE METADATA & CORE PROPERTIES")
                            .size(11.5)
                            .strong()
                            .color(theme::TEXT_MUTED),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        Grid::new("raw_pw_table")
                            .num_columns(3)
                            .striped(true)
                            .spacing([18.0, 5.0])
                            .show(ui, |ui| {
                                for (k, v) in &rt.raw_settings_metadata {
                                    ui.label(
                                        egui::RichText::new("settings")
                                            .color(theme::ACCENT_BLUE)
                                            .size(11.5),
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
                                            .size(11.5),
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
                                            .size(11.5),
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
                });
            }
        });

        reload_all_clicked
    }
}

fn draw_live_status_card(
    ui: &mut egui::Ui,
    status: Option<&DaemonStatus>,
    runtime: Option<&PwRuntimeState>,
    config: Option<&Config>,
) {
    theme::wide_card(ui, |ui| {
        theme::section_label(ui, "PipeWire Server & Živý Stav");
        ui.add_space(8.0);
        if let Some(rt) = runtime {
            if let Some((q, r, ms)) = rt.effective_latency() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{ms:.2} ms"))
                            .size(24.0)
                            .color(theme::ECONOMY_GREEN)
                            .strong(),
                    )
                    .on_hover_text(
                        "Zpoždění zpracování zvuku v jednom bufferu (quantum / frekvence). Do ~5 ms vhodné pro hry a DAW, 10–21 ms je úsporné pro filmy a běžný poslech.",
                    );
                    ui.label(
                        egui::RichText::new(format!("efektivní latence ({q} vzorků @ {r} Hz)"))
                            .size(13.0)
                            .color(theme::TEXT_MUTED),
                    );
                });
                ui.add_space(10.0);
            }

            let active_quantum = format_pw_value(
                rt.clock_force_quantum.as_deref(),
                rt.clock_quantum.as_deref(),
            );
            let active_rate =
                format_pw_value(rt.clock_force_rate.as_deref(), rt.clock_rate.as_deref());
            let saved_pref = config
                .is_some_and(|c| c.dsp.default_quantum.is_some() || c.dsp.default_rate.is_some());
            let profile = status
                .and_then(|s| s.dsp_active_profile.as_deref())
                .unwrap_or(if saved_pref {
                    "uložená preference (rules.toml)"
                } else {
                    "výchozí / auto"
                });
            let sink = rt.default_audio_sink.as_deref().unwrap_or("-");

            ui.columns(2, |cols| {
                cols[0].vertical(|ui| {
                    kv(
                        ui,
                        "PipeWire Verze",
                        rt.core_version.as_deref().unwrap_or("-"),
                        "Verze běžícího PipeWire audio serveru v systému.",
                    );
                    ui.add_space(8.0);
                    kv(
                        ui,
                        "Aktivní Quantum",
                        &active_quantum,
                        "Počet zvukových vzorků zpracovaných najednou. Menší = nižší latence, větší = stabilnější.",
                    );
                    ui.add_space(8.0);
                    kv(
                        ui,
                        "Crimp Profil",
                        profile,
                        "Automatický DSP profil podle spuštěné aplikace (nebo ruční přepínač vpravo).",
                    );
                });
                cols[1].vertical(|ui| {
                    kv(
                        ui,
                        "Core",
                        rt.core_name.as_deref().unwrap_or("-"),
                        "Název hlavní instance PipeWire serveru.",
                    );
                    ui.add_space(8.0);
                    kv(
                        ui,
                        "Aktivní Sample Rate",
                        &active_rate,
                        "Vzorkovací frekvence grafu v Hz (48000 Hz standard, 96000 Hz studio).",
                    );
                    ui.add_space(8.0);
                    kv(
                        ui,
                        "Výchozí Sink",
                        sink,
                        "Výchozí výstupní zařízení (sluchátka/repro), kam míří nově spuštěné aplikace.",
                    );
                });
            });
        } else {
            ui.label(
                egui::RichText::new("PipeWire Runtime data nejsou k dispozici.")
                    .color(theme::TEXT_MUTED),
            );
        }
    });
}

fn draw_dsp_card(
    ui: &mut egui::Ui,
    status: Option<&DaemonStatus>,
    config: Option<&Config>,
    client: &CrimpClient,
) {
    let selected_quantum = status
        .and_then(|s| s.manual_quantum_override)
        .or_else(|| config.and_then(|c| c.dsp.default_quantum));
    let selected_rate = status
        .and_then(|s| s.manual_rate_override)
        .or_else(|| config.and_then(|c| c.dsp.default_rate));

    theme::wide_card(ui, |ui| {
        theme::section_label(ui, "Živé DSP Přepínače (Buffer / Sample Rate)");
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Projeví se okamžitě v PipeWire bez restartu aplikací a rovnou se uloží do rules.toml (načtou se při každém startu PC).",
            )
            .size(12.0)
            .color(theme::TEXT_MUTED),
        );
        ui.add_space(10.0);

        ui.label(
            egui::RichText::new("Quantum (Velikost bufferu):")
                .size(12.5)
                .color(theme::TEXT_MUTED),
        )
        .on_hover_text(
            "64–128 = nízká latence pro DAW a kompetitivní hry.\n256–512 = ideální zlatý střed.\n1024–2048 = minimální spotřeba CPU, nulové praskání.\nAuto = nechá PipeWire rozhodovat samo.",
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for (q, hint) in [
                (64u32, "Ultra nízká latence (~1.3 ms @ 48k)"),
                (128, "Studio / nízká latence (~2.7 ms @ 48k)"),
                (256, "Hraní / vyvážená latence (~5.3 ms @ 48k)"),
                (512, "Běžné použití (~10.7 ms @ 48k)"),
                (1024, "Úsporný buffer (~21.3 ms @ 48k)"),
                (2048, "Maximální stabilita při extrémní zátěži"),
            ] {
                let active = selected_quantum == Some(q);
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
                .selectable_label(selected_quantum.is_none(), "Auto")
                .on_hover_text("Vrátí řízení velikosti bufferu zpět PipeWire/aplikacím")
                .clicked()
            {
                client.send_command(IpcRequest::SetDsp {
                    quantum: DspOverrideChange::Clear,
                    rate: DspOverrideChange::Keep,
                });
            }
        });

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("Sample Rate (Vzorkovací frekvence):")
                .size(12.5)
                .color(theme::TEXT_MUTED),
        )
        .on_hover_text(
            "48000 Hz je výchozí standard pro Windows, Linux, hry i video.\n44100 Hz používají hudební CD/Spotify.\n96000+ Hz má smysl hlavně pro DACy a hudební produkci.",
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for r in [44100u32, 48000, 88200, 96000, 192000] {
                let active = selected_rate == Some(r);
                if ui.selectable_label(active, format!("{r} Hz")).clicked() {
                    client.send_command(IpcRequest::SetDsp {
                        quantum: DspOverrideChange::Keep,
                        rate: DspOverrideChange::Set(r),
                    });
                }
            }
            if ui
                .selectable_label(selected_rate.is_none(), "Auto")
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
}

fn format_pw_value(forced: Option<&str>, fallback: Option<&str>) -> String {
    let clean_forced = forced.map(str::trim).filter(|s| !s.is_empty() && *s != "0");
    if let Some(v) = clean_forced {
        return format!("{v} (vynuceno)");
    }
    let clean_default = fallback
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "0");
    match clean_default {
        Some(v) => format!("{v} (auto)"),
        None => "-".to_string(),
    }
}

fn status_dot(ui: &mut egui::Ui, enabled: bool, label: &str) {
    let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    let color = if enabled {
        theme::ECONOMY_GREEN
    } else {
        theme::TEXT_SUBTLE
    };
    ui.painter().circle_filled(dot_rect.center(), 3.6, color);
    ui.label(
        egui::RichText::new(format!(
            "{label}: {}",
            if enabled { "aktivní" } else { "vypnuto" }
        ))
        .size(12.5)
        .color(if enabled {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SUBTLE
        }),
    );
}

fn kv(ui: &mut egui::Ui, label: &str, value: &str, tooltip: &str) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.5)
                .color(theme::TEXT_MUTED),
        )
        .on_hover_text(tooltip);
        ui.label(
            egui::RichText::new(value)
                .size(13.5)
                .strong()
                .color(theme::TEXT_PRIMARY),
        )
        .on_hover_text(tooltip);
    });
}

fn sync_input_field(buf: &mut String, dirty: bool, desired: Option<String>) {
    if !dirty {
        *buf = desired.unwrap_or_default();
    }
}

fn parse_opt_u32_limit(key: &str, raw: &str) -> Result<Option<u32>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let val = trimmed
        .parse::<u32>()
        .map_err(|_| format!("Neplatné číslo pro {key}: '{trimmed}'"))?;
    if val == 0 {
        return Err(format!(
            "{key} musí být kladné číslo (nebo prázdné pro Auto)"
        ));
    }
    Ok(Some(val))
}

fn parse_opt_allowed_rates(raw: &str) -> Result<Option<Vec<u32>>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    crimp_proto::config::parse_allowed_rates_list(trimmed).map(Some)
}

#[allow(clippy::too_many_arguments)]
fn setting_row(
    ui: &mut egui::Ui,
    key: &str,
    buf: &mut String,
    dirty: &mut bool,
    hint: &str,
    tooltip: &str,
    target: MetadataTarget,
    client: &CrimpClient,
) {
    ui.label(egui::RichText::new(key).monospace())
        .on_hover_text(tooltip);
    let edit = ui
        .add(
            egui::TextEdit::singleline(buf)
                .desired_width(220.0)
                .hint_text(hint),
        )
        .on_hover_text(tooltip);
    if edit.changed() {
        *dirty = true;
    }
    ui.horizontal(|ui| {
        if ui.button("Použít").clicked() {
            let trimmed = buf.trim();
            let val = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            *dirty = false;
            client.send_command(IpcRequest::SetPwSetting {
                target,
                key: key.to_string(),
                value: val,
            });
        }
        if ui.button("Auto").clicked() {
            buf.clear();
            *dirty = false;
            client.send_command(IpcRequest::SetPwSetting {
                target,
                key: key.to_string(),
                value: None,
            });
        }
    });
    ui.end_row();
}
