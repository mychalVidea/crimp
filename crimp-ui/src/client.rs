use crate::autostart::{self, AutostartState};
use crate::tray;
use crimp_proto::config::Config;
use crimp_proto::graph::{DeniedRecord, GraphSnapshot, PwRuntimeState};
use crimp_proto::ipc::{DaemonStatus, IpcRequest, IpcResponse, resolve_socket_path, send_request};
use crimp_proto::update::{self, UpdateInfo};
use std::path::PathBuf;
use std::process::Command;
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
    Command {
        req: IpcRequest,
        feedback_tx: mpsc::Sender<ActionFeedback>,
    },
    Shutdown,
}

/// Asynchronní klient:
/// - IPC polling běží ve vlastním vlákně (`crimp-ui-ipc`) bez blokování okna,
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
}

impl CrimpClient {
    #[must_use]
    pub fn spawn(ctx: egui::Context) -> Self {
        let socket_path = resolve_socket_path(None);
        let snapshot = Arc::new(Mutex::new(UiSnapshot {
            autostart: autostart::probe_autostart(),
            ..Default::default()
        }));
        let snapshot_bg = Arc::clone(&snapshot);
        let sock_bg = socket_path.clone();

        let (msg_tx, msg_rx) = mpsc::channel::<ClientMsg>();
        let (feedback_tx, feedback_rx) = mpsc::channel::<ActionFeedback>();
        let update_busy = Arc::new(AtomicBool::new(false));
        let ctx_ipc = ctx.clone();
        let fb_ipc = feedback_tx.clone();

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
                loop {
                    poll_daemon(&sock_bg, &snapshot_bg);
                    ctx_ipc.request_repaint();

                    match msg_rx.recv_timeout(Duration::from_millis(750)) {
                        Ok(ClientMsg::RefreshNow) => continue,
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
                        Ok(ClientMsg::Command { req, feedback_tx }) => {
                            let res = match send_request(&sock_bg, &req) {
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
                            poll_daemon(&sock_bg, &snapshot_bg);
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
            req,
            feedback_tx: self.feedback_tx.clone(),
        });
    }

    #[must_use]
    pub fn drain_feedback(&self) -> Option<ActionFeedback> {
        let mut latest = None;
        while let Ok(item) = self.feedback_rx.try_recv() {
            latest = Some(item);
        }
        latest
    }

    pub fn spawn_background_daemon(&self) -> Result<(), String> {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("crimp")));
        let binary = sibling
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("crimp"));

        Command::new(&binary)
            .arg("daemon")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Nelze spustit '{}': {e}", binary.display()))
    }

    pub fn restart_gui_process(&self) -> Result<(), String> {
        tray::cleanup_window_socket();
        tray::restart_tray_if_running();
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Command::new(exe)
            .arg("--new-window")
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Nelze znovu spustit UI: {e}"))
    }
}

impl Drop for CrimpClient {
    fn drop(&mut self) {
        let _ = self.msg_tx.send(ClientMsg::Shutdown);
    }
}

fn poll_daemon(socket_path: &PathBuf, out: &Arc<Mutex<UiSnapshot>>) {
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

    if let Ok(mut guard) = out.lock() {
        guard.connected = true;
        guard.last_error = None;
        guard.status = Some(status);
        guard.graph = graph;
        guard.pw_settings = pw_settings;
        guard.autostart = autostart;
        if config.is_some() {
            guard.config = config;
        }
        guard.denied_history = denied_history;
    }
}
