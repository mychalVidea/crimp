mod client;
mod patchbay;
mod rules_view;
mod settings_view;
mod theme;

use client::{ActionFeedback, CrimpClient};
use crimp_proto::ipc::IpcRequest;
use egui::Stroke;
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
                    theme::spec_divider(ui);

                    if snap.connected {
                        ui.colored_label(theme::ECONOMY_GREEN, "Daemon aktivní");
                    } else {
                        ui.colored_label(theme::DANGER_RED, "Daemon odpojen");
                        ui.label(
                            egui::RichText::new(format!(
                                "({})",
                                self.client.socket_path().display()
                            ))
                            .size(11.0)
                            .color(theme::TEXT_MUTED),
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
                                egui::RichText::new(format!("PW {ver}")).color(theme::TEXT_MUTED),
                            );
                        }
                        if let Some((q, r, ms)) = rt.effective_latency() {
                            theme::spec_divider(ui);
                            ui.label(
                                egui::RichText::new(format!("{q} @ {r} Hz ({ms:.2} ms)"))
                                    .color(theme::TEXT_PRIMARY),
                            );
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Reload").clicked() {
                            self.client.send_command(IpcRequest::Reload);
                        }
                        if ui.button("Obnovit").clicked() {
                            self.client.refresh_now();
                        }
                        theme::spec_divider(ui);

                        if ui
                            .selectable_label(self.tab == ActiveTab::Rules, "Pravidla & Blocklist")
                            .clicked()
                        {
                            self.tab = ActiveTab::Rules;
                        }
                        if ui
                            .selectable_label(self.tab == ActiveTab::Settings, "PipeWire Settings")
                            .clicked()
                        {
                            self.tab = ActiveTab::Settings;
                        }
                        if ui
                            .selectable_label(self.tab == ActiveTab::Patchbay, "Patchbay")
                            .clicked()
                        {
                            self.tab = ActiveTab::Patchbay;
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
                        .inner_margin(egui::Margin::symmetric(14, 6)),
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
