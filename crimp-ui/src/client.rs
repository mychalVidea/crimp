use crate::autostart::{self, AutostartState};
use crate::tray;
use crimp_proto::config::Config;
use crimp_proto::graph::{DeniedRecord, GraphSnapshot, PwRuntimeState};
use crimp_proto::ipc::{DaemonStatus, IpcRequest, IpcResponse, resolve_socket_path, send_request};
use crimp_proto::scenes::{self, SceneInfo};
use crimp_proto::telemetry::{self, AudioHealthSnapshot, NodeVolumeState};
use crimp_proto::update::{self, UpdateInfo};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct UiSnapshot {
    pub connected: bool,
    pub last_error: Option<String>,
    pub status: Option<DaemonStatus>,
    pub graph: Option<GraphSnapshot>,
    pub pw_settings: Option<PwRuntimeState>,
    pub config: Option<Config>,
    pub denied_history: Vec<DeniedRecord>,
    pub autostart: AutostartState,
    pub audio_health: AudioHealthSnapshot,
    pub volumes: BTreeMap<u32, NodeVolumeState>,
    pub scenes: Vec<SceneInfo>,
    pub active_scene_id: Option<String>,
    pub update_info: Option<UpdateInfo>,
    pub update_in_progress: bool,
    pub restart_ready: bool,
}

#[derive(Debug)]
pub struct ActionFeedback {
    pub is_error: bool,
    pub message: String,
}

enum ClientMsg {
    RefreshNow,
    SetAutostart(bool),
    SetNodeMute {
        node_id: u32,
        muted: bool,
    },
    SetNodeVolume {
        node_id: u32,
        volume: f32,
    },
    SaveScene {
        name: String,
        config: Box<Config>,
    },
    ApplyScene {
        scene_id: String,
    },
    DeleteScene {
        scene_id: String,
    },
    RefreshScenes,
    Command {
        req: Box<IpcRequest>,
        feedback_tx: mpsc::Sender<ActionFeedback>,
    },
    Shutdown,
}

/// Asynchronní klient:
/// - IPC polling běží ve vlastním vlákně (`crimp-ui-ipc`) bez blokování okna,
/// - živé xruny a aktivita uzlů běží ve vlákně `crimp-ui-telemetry` (`pw-top -b`),
/// - kontrola & instalace aktualizací běží v samostatném worker vlákně (`crimp-ui-update`),
///   takže GUI nezamrzne ani při stahování nebo komunikaci s GitHubem.
pub struct CrimpClient {
    socket_path: PathBuf,
    ctx: egui::Context,
    snapshot: Arc<Mutex<UiSnapshot>>,
    msg_tx: mpsc::Sender<ClientMsg>,
    feedback_rx: mpsc::Receiver<ActionFeedback>,
    feedback_tx: mpsc::Sender<ActionFeedback>,
    update_busy: Arc<AtomicBool>,
    telemetry_stop: Arc<AtomicBool>,
    pw_top_child: Arc<Mutex<Option<Child>>>,
}

impl CrimpClient {
    #[must_use]
    pub fn spawn(ctx: egui::Context) -> Self {
        let socket_path = resolve_socket_path(None);
        let snapshot = Arc::new(Mutex::new(UiSnapshot {
            autostart: autostart::probe_autostart(),
            scenes: scenes::list_scenes(),
            ..Default::default()
        }));
        let snapshot_bg = Arc::clone(&snapshot);
        let sock_bg = socket_path.clone();

        let (msg_tx, msg_rx) = mpsc::channel::<ClientMsg>();
        let (feedback_tx, feedback_rx) = mpsc::channel::<ActionFeedback>();
        let update_busy = Arc::new(AtomicBool::new(false));
        let telemetry_stop = Arc::new(AtomicBool::new(false));
        let pw_top_child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let ctx_ipc = ctx.clone();
        let fb_ipc = feedback_tx.clone();

        // Živá telemetrie PipeWire (`pw-top -b`): xruny + aktivita uzlů (R/I/S + B/Q)
        spawn_pw_top_telemetry_thread(
            Arc::clone(&snapshot),
            ctx.clone(),
            Arc::clone(&telemetry_stop),
            Arc::clone(&pw_top_child),
        );

        // Tichá počáteční kontrola prepackage verze mimo hlavní smyčku i mimo IPC polling
        let snap_init = Arc::clone(&snapshot);
        let ctx_init = ctx.clone();
        let _ = thread::Builder::new()
            .name("crimp-ui-ver".into())
            .spawn(move || {
                if let Ok(info) = update::check_for_updates()
                    && let Ok(mut guard) = snap_init.lock()
                {
                    guard.update_info = Some(info);
                    ctx_init.request_repaint();
                }
            });

        let _ = thread::Builder::new()
            .name("crimp-ui-ipc".into())
            .spawn(move || {
                if send_request(&sock_bg, &IpcRequest::Ping).is_err()
                    && (autostart::probe_autostart().daemon_enabled
                        || crimp_proto::config::default_config_path().is_ok_and(|p| p.exists()))
                    && ensure_background_daemon_started().is_ok()
                {
                    thread::sleep(Duration::from_millis(250));
                }

                let mut poll_tick: u64 = 0;
                loop {
                    poll_daemon(&sock_bg, &snapshot_bg, poll_tick.is_multiple_of(2));
                    poll_tick = poll_tick.wrapping_add(1);
                    ctx_ipc.request_repaint();

                    match msg_rx.recv_timeout(Duration::from_millis(750)) {
                        Ok(ClientMsg::RefreshNow) => {
                            poll_tick = 0;
                            continue;
                        }
                        Ok(ClientMsg::SetAutostart(enable)) => {
                            let res = match autostart::set_autostart(enable) {
                                Ok(msg) => ActionFeedback {
                                    is_error: false,
                                    message: msg,
                                },
                                Err(msg) => ActionFeedback {
                                    is_error: true,
                                    message: msg,
                                },
                            };
                            if let Ok(mut guard) = snapshot_bg.lock() {
                                guard.autostart = autostart::probe_autostart();
                            }
                            let _ = fb_ipc.send(res);
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::SetNodeMute { node_id, muted }) => {
                            let arg = if muted { "1" } else { "0" };
                            let status = Command::new("wpctl")
                                .env("LC_ALL", "C")
                                .args(["set-mute", &node_id.to_string(), arg])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status();
                            if status.is_ok_and(|s| s.success()) {
                                refresh_single_node_volume(node_id, &snapshot_bg);
                            } else {
                                let _ = fb_ipc.send(ActionFeedback {
                                    is_error: true,
                                    message: format!(
                                        "Nelze přepnout ztlumení uzlu #{node_id} (vyžaduje wpctl / WirePlumber)"
                                    ),
                                });
                            }
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::SetNodeVolume { node_id, volume }) => {
                            let clamped = volume.clamp(0.0, 1.5);
                            let val_arg = format!("{clamped:.2}");
                            let status = Command::new("wpctl")
                                .env("LC_ALL", "C")
                                .args(["set-volume", &node_id.to_string(), &val_arg])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status();
                            if status.is_ok_and(|s| s.success()) {
                                refresh_single_node_volume(node_id, &snapshot_bg);
                            } else {
                                let _ = fb_ipc.send(ActionFeedback {
                                    is_error: true,
                                    message: format!(
                                        "Nelze nastavit hlasitost uzlu #{node_id} (vyžaduje wpctl / WirePlumber)"
                                    ),
                                });
                            }
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::SaveScene { name, config }) => {
                            let fb = match scenes::save_scene(&name, config.as_ref()) {
                                Ok(info) => {
                                    if let Ok(mut guard) = snapshot_bg.lock() {
                                        guard.scenes = scenes::list_scenes();
                                        guard.active_scene_id = Some(info.id.clone());
                                    }
                                    tray::refresh_tray_menu();
                                    ActionFeedback {
                                        is_error: false,
                                        message: format!("Scéna '{}' uložena", info.name),
                                    }
                                }
                                Err(err) => ActionFeedback {
                                    is_error: true,
                                    message: err.to_string(),
                                },
                            };
                            let _ = fb_ipc.send(fb);
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::ApplyScene { scene_id }) => {
                            let fb = match scenes::load_scene(&scene_id) {
                                Ok((scene_name, cfg)) => {
                                    match send_request(
                                        &sock_bg,
                                        &IpcRequest::SaveConfig { config: cfg },
                                    ) {
                                        Ok(IpcResponse::Ok { .. }) => {
                                            if let Ok(mut guard) = snapshot_bg.lock() {
                                                guard.active_scene_id = Some(scene_id);
                                            }
                                            poll_daemon(&sock_bg, &snapshot_bg, true);
                                            ActionFeedback {
                                                is_error: false,
                                                message: format!(
                                                    "Scéna '{scene_name}' aktivována"
                                                ),
                                            }
                                        }
                                        Ok(IpcResponse::Error { message }) => ActionFeedback {
                                            is_error: true,
                                            message,
                                        },
                                        Ok(other) => ActionFeedback {
                                            is_error: false,
                                            message: format!("Provedeno: {other:?}"),
                                        },
                                        Err(err) => ActionFeedback {
                                            is_error: true,
                                            message: err.to_string(),
                                        },
                                    }
                                }
                                Err(err) => ActionFeedback {
                                    is_error: true,
                                    message: format!("Nelze načíst scénu: {err}"),
                                },
                            };
                            let _ = fb_ipc.send(fb);
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::DeleteScene { scene_id }) => {
                            let fb = match scenes::delete_scene(&scene_id) {
                                Ok(()) => {
                                    if let Ok(mut guard) = snapshot_bg.lock() {
                                        guard.scenes = scenes::list_scenes();
                                        if guard.active_scene_id.as_deref()
                                            == Some(scene_id.as_str())
                                        {
                                            guard.active_scene_id = None;
                                        }
                                    }
                                    tray::refresh_tray_menu();
                                    ActionFeedback {
                                        is_error: false,
                                        message: "Scéna byla smazána".into(),
                                    }
                                }
                                Err(err) => ActionFeedback {
                                    is_error: true,
                                    message: err.to_string(),
                                },
                            };
                            let _ = fb_ipc.send(fb);
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::RefreshScenes) => {
                            if let Ok(mut guard) = snapshot_bg.lock() {
                                guard.scenes = scenes::list_scenes();
                            }
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::Command { req, feedback_tx }) => {
                            let res = match send_request(&sock_bg, req.as_ref()) {
                                Ok(IpcResponse::Ok { message }) => ActionFeedback {
                                    is_error: false,
                                    message,
                                },
                                Ok(IpcResponse::Error { message }) => ActionFeedback {
                                    is_error: true,
                                    message,
                                },
                                Ok(other) => ActionFeedback {
                                    is_error: false,
                                    message: format!("Provedeno: {other:?}"),
                                },
                                Err(err) => ActionFeedback {
                                    is_error: true,
                                    message: err.to_string(),
                                },
                            };
                            let _ = feedback_tx.send(res);
                            poll_daemon(&sock_bg, &snapshot_bg, false);
                            ctx_ipc.request_repaint();
                        }
                        Ok(ClientMsg::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            });

        Self {
            socket_path,
            ctx,
            snapshot,
            msg_tx,
            feedback_rx,
            feedback_tx,
            update_busy,
            telemetry_stop,
            pw_top_child,
        }
    }

    #[must_use]
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    #[must_use]
    pub fn snapshot(&self) -> UiSnapshot {
        self.snapshot.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn refresh_now(&self) {
        let _ = self.msg_tx.send(ClientMsg::RefreshNow);
    }

    pub fn set_autostart(&self, enable: bool) {
        let _ = self.msg_tx.send(ClientMsg::SetAutostart(enable));
    }

    pub fn set_node_mute(&self, node_id: u32, muted: bool) {
        if let Ok(mut guard) = self.snapshot.lock() {
            let entry = guard.volumes.entry(node_id).or_default();
            entry.muted = muted;
        }
        let _ = self.msg_tx.send(ClientMsg::SetNodeMute { node_id, muted });
        self.ctx.request_repaint();
    }

    pub fn set_node_volume(&self, node_id: u32, volume: f32) {
        let clamped = volume.clamp(0.0, 1.5);
        if let Ok(mut guard) = self.snapshot.lock() {
            let entry = guard.volumes.entry(node_id).or_default();
            entry.volume = clamped;
        }
        let _ = self.msg_tx.send(ClientMsg::SetNodeVolume {
            node_id,
            volume: clamped,
        });
        self.ctx.request_repaint();
    }

    pub fn save_scene(&self, name: String, config: Config) {
        let _ = self.msg_tx.send(ClientMsg::SaveScene {
            name,
            config: Box::new(config),
        });
    }

    pub fn apply_scene(&self, scene_id: String) {
        let _ = self.msg_tx.send(ClientMsg::ApplyScene { scene_id });
    }

    pub fn delete_scene(&self, scene_id: String) {
        let _ = self.msg_tx.send(ClientMsg::DeleteScene { scene_id });
    }

    pub fn refresh_scenes(&self) {
        let _ = self.msg_tx.send(ClientMsg::RefreshScenes);
    }

    /// Zkontroluje dostupnost aktualizace na samostatném vlákně (nikdy neblokuje GUI).
    pub fn check_updates(&self) {
        if self.update_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let snap = Arc::clone(&self.snapshot);
        let ctx = self.ctx.clone();
        let tx = self.feedback_tx.clone();
        let busy = Arc::clone(&self.update_busy);

        let _ = thread::Builder::new()
            .name("crimp-ui-updchk".into())
            .spawn(move || {
                let res = match update::check_for_updates() {
                    Ok(info) => {
                        let msg = if info.update_available {
                            format!(
                                "Nová verze k dispozici: v{} ({}) — klikni na Aktualizovat.",
                                info.latest_version, info.source_label
                            )
                        } else {
                            let c = info
                                .current_commit
                                .as_deref()
                                .map(|s| format!(" · {s}"))
                                .unwrap_or_default();
                            format!("Crimp je aktuální (v{}{c}).", info.current_version)
                        };
                        if let Ok(mut g) = snap.lock() {
                            g.update_info = Some(info);
                        }
                        ActionFeedback {
                            is_error: false,
                            message: msg,
                        }
                    }
                    Err(err) => ActionFeedback {
                        is_error: true,
                        message: err,
                    },
                };
                busy.store(false, Ordering::SeqCst);
                let _ = tx.send(res);
                ctx.request_repaint();
            });
    }

    /// Nejprve ověří verzi na GitHubu; jen pokud existuje vyšší verze/commit, stáhne a nainstaluje prepackage.
    /// Jinak rovnou oznámí, že je aktuální (bez zbytečného stahování i bez sekání okna).
    pub fn check_and_update(&self) {
        if self.update_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut g) = self.snapshot.lock() {
            g.update_in_progress = true;
        }
        let snap = Arc::clone(&self.snapshot);
        let ctx = self.ctx.clone();
        let tx = self.feedback_tx.clone();
        let busy = Arc::clone(&self.update_busy);

        let _ = tx.send(ActionFeedback {
            is_error: false,
            message: "Ověřuji dostupnost aktualizace na GitHubu...".into(),
        });
        ctx.request_repaint();

        let _ = thread::Builder::new()
            .name("crimp-ui-update".into())
            .spawn(move || {
                let outcome = match update::check_for_updates() {
                    Ok(info) => {
                        let up_to_date = !info.update_available;
                        if let Ok(mut g) = snap.lock() {
                            g.update_info = Some(info.clone());
                        }
                        if up_to_date {
                            let c = info
                                .current_commit
                                .as_deref()
                                .map(|s| format!(" · {s}"))
                                .unwrap_or_default();
                            ActionFeedback {
                                is_error: false,
                                message: format!(
                                    "Crimp je aktuální (v{}{c}).",
                                    info.current_version
                                ),
                            }
                        } else {
                            let _ = tx.send(ActionFeedback {
                                is_error: false,
                                message: format!(
                                    "Stahuji aktualizaci v{} ({})...",
                                    info.latest_version, info.source_label
                                ),
                            });
                            ctx.request_repaint();

                            match update::run_self_update() {
                                Ok(msg) => {
                                    if let Ok(mut g) = snap.lock() {
                                        g.restart_ready = true;
                                    }
                                    ActionFeedback {
                                        is_error: false,
                                        message: msg,
                                    }
                                }
                                Err(err) => ActionFeedback {
                                    is_error: true,
                                    message: err,
                                },
                            }
                        }
                    }
                    Err(err) => ActionFeedback {
                        is_error: true,
                        message: err,
                    },
                };

                if let Ok(mut g) = snap.lock() {
                    g.update_in_progress = false;
                }
                busy.store(false, Ordering::SeqCst);
                let _ = tx.send(outcome);
                ctx.request_repaint();
            });
    }

    pub fn send_command(&self, req: IpcRequest) {
        let _ = self.msg_tx.send(ClientMsg::Command {
            req: Box::new(req),
            feedback_tx: self.feedback_tx.clone(),
        });
    }

    pub fn push_feedback(&self, is_error: bool, message: impl Into<String>) {
        let _ = self.feedback_tx.send(ActionFeedback {
            is_error,
            message: message.into(),
        });
        self.ctx.request_repaint();
    }

    #[must_use]
    pub fn drain_feedback(&self) -> Option<ActionFeedback> {
        let mut latest = None;
        let mut latest_error = None;
        while let Ok(item) = self.feedback_rx.try_recv() {
            if item.is_error {
                latest_error = Some(item);
            } else {
                latest = Some(item);
            }
        }
        latest_error.or(latest)
    }

    pub fn spawn_background_daemon(&self) -> Result<(), String> {
        ensure_background_daemon_started()
    }

    pub fn restart_gui_process(&self) -> Result<(), String> {
        tray::cleanup_window_socket();
        tray::restart_tray_if_running();
        let exe = update::resolve_live_binary("crimp-ui");
        Command::new(&exe)
            .arg("--new-window")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Nelze znovu spustit UI ({}): {e}", exe.display()))
    }
}

impl Drop for CrimpClient {
    fn drop(&mut self) {
        self.telemetry_stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.pw_top_child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.msg_tx.send(ClientMsg::Shutdown);
    }
}

fn spawn_pw_top_telemetry_thread(
    snapshot: Arc<Mutex<UiSnapshot>>,
    ctx: egui::Context,
    stop: Arc<AtomicBool>,
    child_slot: Arc<Mutex<Option<Child>>>,
) {
    let _ = thread::Builder::new()
        .name("crimp-ui-telemetry".into())
        .spawn(move || {
            let Ok(mut child) = Command::new("pw-top")
                .env("LC_ALL", "C")
                .arg("-b")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return;
            };
            if let Ok(mut slot) = child_slot.lock() {
                *slot = Some(child);
            }

            let reader = BufReader::new(stdout);
            let mut batch = String::new();
            for line_res in reader.lines() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(line) = line_res else {
                    break;
                };
                let is_header =
                    line.starts_with('S') && line.contains("ID") && line.contains("QUANT");
                if is_header && !batch.is_empty() {
                    let parsed = telemetry::parse_pw_top_batch(&batch);
                    if let Ok(mut guard) = snapshot.lock() {
                        guard.audio_health = parsed;
                    }
                    ctx.request_repaint();
                    batch.clear();
                }
                batch.push_str(&line);
                batch.push('\n');
            }

            if let Ok(mut slot) = child_slot.lock()
                && let Some(mut c) = slot.take()
            {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
}

fn poll_wpctl_volumes() -> Option<BTreeMap<u32, NodeVolumeState>> {
    let out = Command::new("wpctl")
        .env("LC_ALL", "C")
        .arg("status")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(telemetry::parse_wpctl_status_volumes(&text))
}

fn refresh_single_node_volume(node_id: u32, out: &Arc<Mutex<UiSnapshot>>) {
    if let Ok(res) = Command::new("wpctl")
        .env("LC_ALL", "C")
        .args(["get-volume", &node_id.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        && res.status.success()
        && let Some(vol) = telemetry::parse_wpctl_get_volume(&String::from_utf8_lossy(&res.stdout))
        && let Ok(mut guard) = out.lock()
    {
        guard.volumes.insert(node_id, vol);
    }
}

fn ensure_background_daemon_started() -> Result<(), String> {
    let systemd_unit_exists = Command::new("systemctl")
        .args(["--user", "cat", "crimp.service"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());

    if systemd_unit_exists
        && Command::new("systemctl")
            .args(["--user", "start", "crimp.service"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    {
        return Ok(());
    }

    let binary = update::resolve_live_binary("crimp");
    Command::new(&binary)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Nelze spustit '{}': {e}", binary.display()))
}

fn poll_daemon(socket_path: &Path, out: &Arc<Mutex<UiSnapshot>>, poll_volumes: bool) {
    let status = match send_request(socket_path, &IpcRequest::Status) {
        Ok(IpcResponse::Status { data }) => data,
        Ok(IpcResponse::Error { message }) => {
            if let Ok(mut guard) = out.lock() {
                guard.connected = false;
                guard.last_error = Some(message);
            }
            return;
        }
        Ok(_) => return,
        Err(err) => {
            if let Ok(mut guard) = out.lock() {
                guard.connected = false;
                guard.last_error = Some(err.to_string());
                guard.status = None;
                guard.graph = None;
                guard.pw_settings = None;
            }
            return;
        }
    };

    let graph = match send_request(socket_path, &IpcRequest::Graph) {
        Ok(IpcResponse::Graph { data }) => Some(data),
        _ => None,
    };
    let pw_settings = match send_request(socket_path, &IpcRequest::PwSettings) {
        Ok(IpcResponse::PwSettings { data }) => Some(data),
        _ => None,
    };
    let config = match send_request(socket_path, &IpcRequest::GetConfig) {
        Ok(IpcResponse::Config { data }) => Some(data),
        _ => None,
    };
    let denied_history = match send_request(socket_path, &IpcRequest::DeniedHistory) {
        Ok(IpcResponse::DeniedHistory { entries }) => entries,
        _ => Vec::new(),
    };

    let autostart = autostart::probe_autostart();
    let volumes = if poll_volumes {
        poll_wpctl_volumes()
    } else {
        None
    };

    if let Ok(mut guard) = out.lock() {
        guard.connected = true;
        guard.last_error = None;
        guard.status = Some(status);
        guard.graph = graph;
        guard.pw_settings = pw_settings;
        guard.autostart = autostart;
        if let Some(vols) = volumes {
            guard.volumes = vols;
        }
        if config.is_some() {
            guard.config = config;
        }
        guard.denied_history = denied_history;
    }
}
