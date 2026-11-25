mod client;
mod patchbay;
mod rules_view;
mod settings_view;
mod theme;

use client::{ActionFeedback, CrimpClient};
use crimp_proto::ipc::IpcRequest;
use egui::{CornerRadius, Stroke};
use patchbay::PatchbayState;
use rules_view::RulesViewState;
use settings_view::SettingsViewState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Patchbay,
    Settings,
    Rules,
}

struct CrimpGuiApp {
    client: CrimpClient,
    tab: ActiveTab,
    patchbay: PatchbayState,
    settings: SettingsViewState,
    rules: RulesViewState,
    last_feedback: Option<ActionFeedback>,
}

impl CrimpGuiApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_obsidian_style(&cc.egui_ctx);
        let client = CrimpClient::spawn(cc.egui_ctx.clone());
        Self {
            client,
            tab: ActiveTab::Patchbay,
            patchbay: PatchbayState::default(),
            settings: SettingsViewState::default(),
            rules: RulesViewState::default(),
            last_feedback: None,
        }
    }
}

impl eframe::App for CrimpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(fb) = self.client.drain_feedback() {
            self.last_feedback = Some(fb);
        }

        let snap = self.client.snapshot();

        egui::Panel::top("crimp_top_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_SECONDARY)
                    .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("CRIMP")
                            .size(16.0)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(theme::TEXT_SUBTLE),
                    );
                    theme::spec_divider(ui);

                    if snap.connected {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot_rect.center(), 3.8, theme::ECONOMY_GREEN);
                        ui.label(
                            egui::RichText::new("Aktivní")
                                .color(theme::ECONOMY_GREEN)
                                .size(12.5),
                        );
                    } else {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot_rect.center(), 3.8, theme::DANGER_RED);
                        ui.label(
                            egui::RichText::new("Odpojeno")
                                .color(theme::DANGER_RED)
                                .size(12.5),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "({})",
                                self.client.socket_path().display()
                            ))
                            .size(11.0)
                            .color(theme::TEXT_SUBTLE),
                        );
                        if ui.button("Spustit daemon").clicked() {
                            match self.client.spawn_background_daemon() {
                                Ok(()) => {
                                    self.last_feedback = Some(ActionFeedback {
                                        is_error: false,
                                        message: "Spouštím Crimp daemon...".into(),
                                    });
                                    self.client.refresh_now();
                                }
                                Err(message) => {
                                    self.last_feedback = Some(ActionFeedback {
                                        is_error: true,
                                        message,
                                    });
                                }
                            }
                        }
                    }

                    if let Some(rt) = &snap.pw_settings {
                        theme::spec_divider(ui);
                        if let Some(ver) = &rt.core_version {
                            ui.label(
                                egui::RichText::new(format!("PipeWire {ver}"))
                                    .color(theme::TEXT_MUTED)
                                    .size(12.5),
                            );
                        }
                        if let Some((q, r, ms)) = rt.effective_latency() {
                            theme::spec_divider(ui);
                            ui.label(
                                egui::RichText::new(format!("{q} @ {r} Hz  ·  {ms:.2} ms"))
                                    .color(theme::TEXT_PRIMARY)
                                    .size(12.5),
                            );
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Akce & Auto-update napravo
                        if snap.restart_ready {
                            if ui
                                .add(
                                    egui::Button::new("Restartovat UI")
                                        .fill(theme::ACCENT_BLUE)
                                        .corner_radius(CornerRadius::same(7)),
                                )
                                .clicked()
                                && self.client.restart_gui_process().is_ok()
                            {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else if snap.update_in_progress {
                            ui.label(
                                egui::RichText::new("Aktualizuji...")
                                    .color(theme::ACCENT_BLUE)
                                    .size(12.0),
                            );
                        } else if let Some(upd) = &snap.update_info
                            && upd.update_available
                        {
                            if ui
                                .add(
                                    egui::Button::new(format!(
                                        "Aktualizovat na v{}",
                                        upd.latest_version
                                    ))
                                    .fill(theme::ACCENT_BLUE)
                                    .corner_radius(CornerRadius::same(7)),
                                )
                                .clicked()
                            {
                                self.client.trigger_self_update();
                            }
                        } else {
                            let update_resp = ui.button("Update").on_hover_text(
                                "Kliknutím zkontroluje / aktualizuje Crimp z GitHubu",
                            );
                            if update_resp.clicked() {
                                self.client.trigger_self_update();
                            }
                            if update_resp.secondary_clicked() {
                                self.client.check_updates();
                            }
                        }

                        if ui.button("Reload").clicked() {
                            self.client.send_command(IpcRequest::Reload);
                        }
                        if ui.button("Obnovit").clicked() {
                            self.client.refresh_now();
                        }
                        theme::spec_divider(ui);

                        // Segmentový přepínač záložek
                        for (tab, label) in [
                            (ActiveTab::Rules, "Pravidla & Blocklist"),
                            (ActiveTab::Settings, "PipeWire Settings"),
                            (ActiveTab::Patchbay, "Patchbay"),
                        ] {
                            let active = self.tab == tab;
                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .color(if active {
                                        egui::Color32::WHITE
                                    } else {
                                        theme::TEXT_MUTED
                                    })
                                    .size(12.5),
                            )
                            .fill(if active {
                                theme::ACCENT_BLUE
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(7));

                            if ui.add(btn).clicked() {
                                self.tab = tab;
                            }
                        }
                    });
                });
            });

        if let Some(fb) = &self.last_feedback {
            egui::Panel::bottom("crimp_status_bar")
                .frame(
                    egui::Frame::new()
                        .fill(theme::BG_SECONDARY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .inner_margin(egui::Margin::symmetric(14, 7)),
                )
                .show(ui, |ui| {
                    let color = if fb.is_error {
                        theme::DANGER_RED
                    } else {
                        theme::ECONOMY_GREEN
                    };
                    ui.colored_label(color, &fb.message);
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_OBSIDIAN)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| match self.tab {
                ActiveTab::Patchbay => {
                    self.patchbay
                        .show(ui, snap.graph.as_ref(), snap.config.as_ref(), &self.client);
                }
                ActiveTab::Settings => {
                    self.settings.show(
                        ui,
                        snap.status.as_ref(),
                        snap.pw_settings.as_ref(),
                        snap.graph.as_ref(),
                        snap.config.as_ref(),
                        &self.client,
                    );
                }
                ActiveTab::Rules => {
                    self.rules.show(
                        ui,
                        snap.config.as_ref(),
                        snap.graph.as_ref(),
                        &snap.denied_history,
                        &self.client,
                    );
                }
            });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("crimp")
            .with_inner_size([1240.0, 760.0])
            .with_min_inner_size([840.0, 520.0])
            .with_title("Crimp – PipeWire Patchbay & Settings"),
        ..Default::default()
    };

    eframe::run_native(
        "Crimp",
        options,
        Box::new(|cc| Ok(Box::new(CrimpGuiApp::new(cc)))),
    )
}
