mod autostart;
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tray::WindowWakeGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Patchbay,
    Settings,
    Rules,
}

struct CrimpGuiApp {
    client: CrimpClient,
    _wake_guard: WindowWakeGuard,
    window_quit_flag: Arc<AtomicBool>,
    no_tray: bool,
    restarting: bool,
    tab: ActiveTab,
    patchbay: PatchbayState,
    settings: SettingsViewState,
    rules: RulesViewState,
    last_feedback: Option<ActionFeedback>,
    last_feedback_at: Option<Instant>,
}

impl CrimpGuiApp {
    fn new(cc: &eframe::CreationContext<'_>, no_tray: bool) -> Self {
        theme::apply_obsidian_style(&cc.egui_ctx);
        let client = CrimpClient::spawn(cc.egui_ctx.clone());
        let window_quit_flag = Arc::new(AtomicBool::new(false));
        let wake_guard =
            tray::spawn_window_wake_listener(cc.egui_ctx.clone(), Arc::clone(&window_quit_flag));
        if !no_tray {
            tray::ensure_tray_running();
        }
        Self {
            client,
            _wake_guard: wake_guard,
            window_quit_flag,
            no_tray,
            restarting: false,
            tab: ActiveTab::Patchbay,
            patchbay: PatchbayState::default(),
            settings: SettingsViewState::default(),
            rules: RulesViewState::default(),
            last_feedback: None,
            last_feedback_at: None,
        }
    }

    fn set_feedback(&mut self, fb: ActionFeedback) {
        self.last_feedback = Some(fb);
        self.last_feedback_at = Some(Instant::now());
    }

    fn reload_rules_and_layout(&mut self) {
        self.client.send_command(IpcRequest::Reload);
        self.client.refresh_scenes();
        self.patchbay.reload_layout_from_disk();
        self.settings.reset_dirty_inputs();
    }

    fn close_window_to_tray(&mut self, ctx: &egui::Context, snap: &client::UiSnapshot) {
        self.patchbay.flush_saved_layout(snap.graph.as_ref());
        if !self.no_tray && !self.restarting && !self.window_quit_flag.load(Ordering::SeqCst) {
            tray::ensure_tray_running();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for CrimpGuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.client.snapshot();

        // Na Waylandu `ViewportCommand::Visible(false)` nefunguje; tray běží jako samostatný proces (`--tray`),
        // takže křížek `×` okno skutečně čistě zavře, zatímco tray ikona a daemon zůstanou na pozadí.
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            self.patchbay.flush_saved_layout(snap.graph.as_ref());
            if !self.no_tray && !self.restarting && !self.window_quit_flag.load(Ordering::SeqCst) {
                tray::ensure_tray_running();
            }
        }

        if let Some(fb) = self.client.drain_feedback() {
            self.set_feedback(fb);
        }
        if let Some(fb) = &self.last_feedback
            && !fb.is_error
            && self
                .last_feedback_at
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(6))
        {
            self.last_feedback = None;
            self.last_feedback_at = None;
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
                                    self.set_feedback(ActionFeedback {
                                        is_error: false,
                                        message: "Spouštím Crimp daemon...".into(),
                                    });
                                    self.client.refresh_now();
                                }
                                Err(message) => {
                                    self.set_feedback(ActionFeedback {
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

                    if snap.audio_health.available {
                        theme::spec_divider(ui);
                        let xruns = snap.audio_health.total_xruns;
                        let (xrun_txt, xrun_col) = if xruns == 0 {
                            ("0 xruns".to_string(), theme::ECONOMY_GREEN)
                        } else {
                            (format!("{xruns} xruns"), theme::DANGER_RED)
                        };
                        let mut tip = if xruns == 0 {
                            format!(
                                "Zvuk běží čistě bez výpadků (aktivních uzlů: {}).",
                                snap.audio_health.active_nodes
                            )
                        } else {
                            let offenders: Vec<String> = snap
                                .audio_health
                                .per_node
                                .values()
                                .filter(|m| m.err_count > 0)
                                .take(5)
                                .map(|m| format!("• {} (#{}): {}×", m.name, m.id, m.err_count))
                                .collect();
                            format!(
                                "Detekovány výpadky zvuku (xruns)!\n{}\nTip: zvedni Quantum (např. 256 → 512) v PipeWire Settings.",
                                offenders.join("\n")
                            )
                        };
                        if snap.audio_health.active_nodes == 0 && xruns == 0 {
                            tip = "PipeWire profiler aktivní · žádné výpadky (xruns).".into();
                        }
                        ui.label(egui::RichText::new(xrun_txt).color(xrun_col).size(12.5))
                            .on_hover_text(tip);
                    }

                    if !snap.scenes.is_empty() {
                        theme::spec_divider(ui);
                        let active_label = snap
                            .active_scene_id
                            .as_deref()
                            .and_then(|id| snap.scenes.iter().find(|s| s.id == id))
                            .map_or("Scéna...", |s| s.name.as_str());
                        egui::ComboBox::from_id_salt("top_bar_scene_picker")
                            .selected_text(active_label)
                            .width(125.0)
                            .show_ui(ui, |ui| {
                                for scene in &snap.scenes {
                                    let is_active =
                                        snap.active_scene_id.as_deref() == Some(scene.id.as_str());
                                    if ui.selectable_label(is_active, &scene.name).clicked() {
                                        self.client.apply_scene(scene.id.clone());
                                    }
                                }
                            });
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Do lišty")
                            .on_hover_text(
                                "Zavře toto okno do oznamovací oblasti (system tray), daemon i pravidla běží dál",
                            )
                            .clicked()
                        {
                            self.close_window_to_tray(ui.ctx(), &snap);
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
                                self.restarting = true;
                                self._wake_guard.disarm_cleanup();
                                self.patchbay.flush_saved_layout(snap.graph.as_ref());
                                match self.client.restart_gui_process() {
                                    Ok(()) => {
                                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                                    }
                                    Err(err) => {
                                        self.restarting = false;
                                        self.set_feedback(ActionFeedback {
                                            is_error: true,
                                            message: err,
                                        });
                                    }
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
                                .on_hover_text(
                                    "Stáhne předkompilovaný balíček z GitHub Actions bez kompilace a bez zamrznutí okna.",
                                )
                                .clicked()
                            {
                                self.client.check_and_update();
                            }
                        } else {
                            let update_resp = ui
                                .button("Update")
                                .on_hover_text(
                                    "Levý klik: zkontroluje verzi na GitHubu; jen pokud vyšel novější build, stáhne prepackage.\nPravý klik: jen ověří dostupnost nové verze.",
                                );
                            if update_resp.clicked() {
                                self.client.check_and_update();
                            }
                            if update_resp.secondary_clicked() {
                                self.client.check_updates();
                            }
                        }

                        if ui
                            .button("Načíst pravidla")
                            .on_hover_text(
                                "Okamžitě znovu načte rules.toml v daemonu i uložené rozložení karet",
                            )
                            .clicked()
                        {
                            self.reload_rules_and_layout();
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

        let mut dismiss_feedback = false;
        if let Some(fb) = &self.last_feedback {
            egui::Panel::bottom("crimp_status_bar")
                .frame(
                    egui::Frame::new()
                        .fill(theme::BG_SECONDARY)
                        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE))
                        .inner_margin(egui::Margin::symmetric(14, 7)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let color = if fb.is_error {
                            theme::DANGER_RED
                        } else {
                            theme::ECONOMY_GREEN
                        };
                        ui.colored_label(color, &fb.message);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Skrýt zprávu").clicked() {
                                dismiss_feedback = true;
                            }
                        });
                    });
                });
        }
        if dismiss_feedback {
            self.last_feedback = None;
            self.last_feedback_at = None;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG_OBSIDIAN)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| match self.tab {
                ActiveTab::Patchbay => {
                    self.patchbay.show(
                        ui,
                        snap.graph.as_ref(),
                        snap.config.as_ref(),
                        &snap.audio_health,
                        &snap.volumes,
                        &self.client,
                    );
                }
                ActiveTab::Settings => {
                    if self.settings.show(
                        ui,
                        snap.status.as_ref(),
                        snap.pw_settings.as_ref(),
                        snap.graph.as_ref(),
                        snap.config.as_ref(),
                        &snap.autostart,
                        &self.client,
                    ) {
                        self.reload_rules_and_layout();
                    }
                }
                ActiveTab::Rules => {
                    self.rules.show(
                        ui,
                        snap.config.as_ref(),
                        snap.graph.as_ref(),
                        &snap.denied_history,
                        &snap.scenes,
                        snap.active_scene_id.as_deref(),
                        &self.client,
                    );
                }
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let snap = self.client.snapshot();
        self.patchbay.flush_saved_layout(snap.graph.as_ref());
        if !self.restarting {
            tray::cleanup_window_socket();
        }
    }
}

fn main() -> eframe::Result<()> {
    let mut tray_only = false;
    let mut force_new_window = false;
    let mut no_tray = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Použití: crimp-ui [--window | --new-window | --tray | --no-tray | --version | --help]"
                );
                return Ok(());
            }
            "--version" | "-V" => {
                let commit = crimp_proto::update::CRIMP_BUILD_COMMIT
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default();
                println!("crimp-ui {}{}", env!("CARGO_PKG_VERSION"), commit);
                return Ok(());
            }
            "--tray" => tray_only = true,
            "--new-window" => force_new_window = true,
            "--no-tray" => no_tray = true,
            "--window" => {}
            _ => {}
        }
    }

    if tray_only {
        tray::run_tray_service_blocking();
        return Ok(());
    }

    // Pokud už okno Crimpu běží, vytáhneme ho do popředí místo duplicitní instance.
    if !force_new_window && tray::try_focus_existing_window() {
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
        Box::new(move |cc| Ok(Box::new(CrimpGuiApp::new(cc, no_tray)))),
    )
}
