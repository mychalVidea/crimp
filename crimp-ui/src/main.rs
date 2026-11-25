mod client;
mod layout_store;
mod patchbay;
mod rules_view;
mod settings_view;
mod theme;
mod tray;

use client::{ActionFeedback, CrimpClient};
use crimp_proto::ipc::IpcRequest;
use egui::{CornerRadius, Stroke};
use patchbay::PatchbayState;
use rules_view::RulesViewState;
use settings_view::SettingsViewState;
use tray::{TrayCommand, TrayController};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Patchbay,
    Settings,
    Rules,
}

struct CrimpGuiApp {
    client: CrimpClient,
    tray: TrayController,
    window_visible: bool,
    force_quit: bool,
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
        let tray = TrayController::spawn(cc.egui_ctx.clone());
        Self {
            client,
            tray,
            window_visible: true,
            force_quit: false,
            tab: ActiveTab::Patchbay,
            patchbay: PatchbayState::default(),
            settings: SettingsViewState::default(),
            rules: RulesViewState::default(),
            last_feedback: None,
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        self.window_visible = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context, snap: &client::UiSnapshot) {
        self.patchbay.flush_saved_layout(snap.graph.as_ref());
        self.window_visible = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
}

impl eframe::App for CrimpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.client.snapshot();

        while let Some(cmd) = self.tray.try_recv() {
            match cmd {
                TrayCommand::ShowWindow => self.show_window(ui.ctx()),
                TrayCommand::HideWindow => self.hide_to_tray(ui.ctx(), &snap),
                TrayCommand::ToggleWindow => {
                    if self.window_visible {
                        self.hide_to_tray(ui.ctx(), &snap);
                    } else {
                        self.show_window(ui.ctx());
                    }
                }
                TrayCommand::ReloadRules => {
                    self.client.send_command(IpcRequest::Reload);
                }
                TrayCommand::QuitApp => {
                    self.patchbay.flush_saved_layout(snap.graph.as_ref());
                    self.force_quit = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Zavření okna křížkem minimalizuje okno do system traye (pokud nekliknuto Ukončit v tray menu)
        if !self.force_quit && ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_to_tray(ui.ctx(), &snap);
        }

        if let Some(fb) = self.client.drain_feedback() {
            self.last_feedback = Some(fb);
        }

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
                            egui::RichText::new(format!("({})", self.client.socket_path().display()))
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
                            )
                            .on_hover_text("Aktuální PipeWire buffer (vzorky), sample rate a výsledná latence");
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Do lišty")
                            .on_hover_text("Skryje okno do oznamovací oblasti (system tray), daemon běží dál")
                            .clicked()
                        {
                            self.hide_to_tray(ui.ctx(), &snap);
                        }

                        if snap.restart_ready {
                            if ui
                                .add(
                                    egui::Button::new("Restartovat UI")
                                        .fill(theme::ACCENT_BLUE)
                                        .corner_radius(CornerRadius::same(7)),
                                )
                                .clicked()
                            {
                                self.patchbay.flush_saved_layout(snap.graph.as_ref());
                                if self.client.restart_gui_process().is_ok() {
                                    self.force_quit = true;
                                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                                }
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
                            let update_resp = ui
                                .button("Update")
                                .on_hover_text(
                                    "Levý klik: stáhne a nainstaluje nejnovější Crimp z GitHubu.\nPravý klik: jen ověří, zda vyšla novější verze.",
                                );
                            if update_resp.clicked() {
                                self.client.trigger_self_update();
                            }
                            if update_resp.secondary_clicked() {
                                self.client.check_updates();
                            }
                        }

                        if ui
                            .button("Načíst pravidla")
                            .on_hover_text("Okamžitě znovu načte ~/.config/crimp/rules.toml v běžícím daemonu")
                            .clicked()
                        {
                            self.client.send_command(IpcRequest::Reload);
                        }
                        theme::spec_divider(ui);

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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let snap = self.client.snapshot();
        self.patchbay.flush_saved_layout(snap.graph.as_ref());
    }
}

fn main() -> eframe::Result<()> {
    // Pokud už okno Crimpu běží v system trayi, jen ho vytáhneme do popředí místo duplicitní instance.
    if tray::try_focus_existing_instance() {
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("crimp")
            .with_inner_size([1240.0, 760.0])
            .with_min_inner_size([860.0, 520.0])
            .with_title("Crimp – PipeWire Patchbay & Settings"),
        ..Default::default()
    };

    eframe::run_native(
        "Crimp",
        options,
        Box::new(|cc| Ok(Box::new(CrimpGuiApp::new(cc)))),
    )
}
