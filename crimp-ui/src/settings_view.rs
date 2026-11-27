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
    initialized_from_runtime: bool,
}

impl SettingsViewState {
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
                    draw_live_status_card(&mut cols[0], status, runtime);
                    draw_dsp_card(&mut cols[1], status, client);
                });
            } else {
                draw_live_status_card(ui, status, runtime);
                ui.add_space(12.0);
                draw_dsp_card(ui, status, client);
            }

            ui.add_space(12.0);

            // 2. Výchozí audio zařízení (`default`) - výstupy i vstupy pohodlně vedle sebe
            if let Some(graph) = graph {
                theme::wide_card(ui, |ui| {
                    theme::section_label(ui, "Výchozí Audio Zařízení ('default')");
                    ui.add_space(8.0);
                    let cur_sink = runtime.and_then(|r| r.default_audio_sink.as_deref());
                    let cur_src = runtime.and_then(|r| r.default_audio_source.as_deref());

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
                                        .is_some_and(|c| c.contains("Sink"))
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

                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
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
                            .on_hover_text(
                                "Zapíše min/max/limit quantum do konfigurace, takže přežijí i restart PC",
                            )
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
            let profile = status
                .and_then(|s| s.dsp_active_profile.as_deref())
                .unwrap_or("výchozí / auto");
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

fn draw_dsp_card(ui: &mut egui::Ui, status: Option<&DaemonStatus>, client: &CrimpClient) {
    theme::wide_card(ui, |ui| {
        theme::section_label(ui, "Živé DSP Přepínače (Buffer / Sample Rate)");
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Najeď myší na libovolný prvek pro vysvětlení. Změny se projeví okamžitě bez restartu her ani programů.",
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
            .desired_width(220.0)
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
