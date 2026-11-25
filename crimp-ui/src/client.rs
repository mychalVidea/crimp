use crimp_proto::config::Config;
use crimp_proto::graph::{DeniedRecord, GraphSnapshot, PwRuntimeState};
use crimp_proto::ipc::{DaemonStatus, IpcRequest, IpcResponse, resolve_socket_path, send_request};
use std::path::PathBuf;
use std::process::Command;
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
}

#[derive(Debug)]
pub struct ActionFeedback {
    pub is_error: bool,
    pub message: String,
}

enum ClientMsg {
    RefreshNow,
    Command {
        req: IpcRequest,
        feedback_tx: mpsc::Sender<ActionFeedback>,
    },
    Shutdown,
}

/// Asynchronní IPC klient oddělující I/O na Unix socketu od vykreslovací smyčky `egui`.
pub struct CrimpClient {
    socket_path: PathBuf,
    snapshot: Arc<Mutex<UiSnapshot>>,
    msg_tx: mpsc::Sender<ClientMsg>,
    feedback_rx: mpsc::Receiver<ActionFeedback>,
    feedback_tx: mpsc::Sender<ActionFeedback>,
}

impl CrimpClient {
    #[must_use]
    pub fn spawn(ctx: egui::Context) -> Self {
        let socket_path = resolve_socket_path(None);
        let snapshot = Arc::new(Mutex::new(UiSnapshot::default()));
        let snapshot_bg = Arc::clone(&snapshot);
        let sock_bg = socket_path.clone();

        let (msg_tx, msg_rx) = mpsc::channel::<ClientMsg>();
        let (feedback_tx, feedback_rx) = mpsc::channel::<ActionFeedback>();

        let _ = thread::Builder::new()
            .name("crimp-ui-ipc".into())
            .spawn(move || {
                loop {
                    poll_daemon(&sock_bg, &snapshot_bg);
                    ctx.request_repaint();

                    match msg_rx.recv_timeout(Duration::from_millis(750)) {
                        Ok(ClientMsg::RefreshNow) => continue,
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
                            ctx.request_repaint();
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
            snapshot,
            msg_tx,
            feedback_rx,
            feedback_tx,
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

    if let Ok(mut guard) = out.lock() {
        guard.connected = true;
        guard.last_error = None;
        guard.status = Some(status);
        guard.graph = graph;
        guard.pw_settings = pw_settings;
        if config.is_some() {
            guard.config = config;
        }
        guard.denied_history = denied_history;
    }
}
