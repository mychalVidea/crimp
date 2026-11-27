use crate::config::Config;
use crate::graph::{DeniedRecord, GraphSnapshot, PwRuntimeState};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// Změna manuálního DSP parametru přes IPC/CLI/GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DspOverrideChange {
    Keep,
    Clear,
    Set(u32),
}

/// Cílový globální PipeWire metadata objekt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataTarget {
    Settings,
    Default,
}

/// Typ výchozího audio zařízení v PipeWire `default` metadatach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultDeviceKind {
    Sink,
    Source,
}

/// Požadavek zasílaný po lokálním Unix Domain Socketu (`\n`-oddělený JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum IpcRequest {
    Ping,
    Status,
    Graph,
    PwSettings,
    GetConfig,
    SaveConfig {
        config: Config,
    },
    DeniedHistory,
    Reload,
    SetDsp {
        quantum: DspOverrideChange,
        rate: DspOverrideChange,
    },
    SetPwSetting {
        target: MetadataTarget,
        key: String,
        value: Option<String>,
    },
    SavePwLimits {
        min_quantum: Option<u32>,
        max_quantum: Option<u32>,
        quantum_limit: Option<u32>,
        allowed_rates: Option<Vec<u32>>,
    },
    SetDefaultDevice {
        kind: DefaultDeviceKind,
        node_name: Option<String>,
    },
    CreateLink {
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
        #[serde(default = "default_true")]
        replace_input: bool,
        #[serde(default = "default_true")]
        persist: bool,
    },
    DestroyLink {
        link_id: u32,
        #[serde(default = "default_true")]
        persist: bool,
    },
    Shutdown,
}

const fn default_true() -> bool {
    true
}

/// Souhrnný stav běžícího daemona.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub config_path: String,
    pub socket_path: Option<String>,
    pub node_count: usize,
    pub port_count: usize,
    pub link_count: usize,
    pub virtual_sinks_active: Vec<String>,
    pub deny_rule_count: usize,
    pub route_rule_count: usize,
    pub dsp_active_profile: Option<String>,
    pub dsp_quantum: Option<u32>,
    pub dsp_rate: Option<u32>,
    pub manual_quantum_override: Option<u32>,
    pub manual_rate_override: Option<u32>,
    pub blocked_total: u64,
    pub routes_applied_total: u64,
}

/// Odpověď daemona přes IPC socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum IpcResponse {
    Pong { version: String },
    Status { data: DaemonStatus },
    Graph { data: GraphSnapshot },
    PwSettings { data: PwRuntimeState },
    Config { data: Config },
    DeniedHistory { entries: Vec<DeniedRecord> },
    Ok { message: String },
    Error { message: String },
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("Daemon už pravděpodobně běží na socketu {0}")]
    AlreadyRunning(PathBuf),

    #[error("Daemon neběží nebo nelze otevřít socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("I/O chyba na IPC socketu ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Chyba serializace/parsování IPC zprávy: {0}")]
    Json(#[from] serde_json::Error),
}

#[must_use]
pub fn resolve_socket_path(custom: Option<&str>) -> PathBuf {
    if let Some(raw) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return PathBuf::from(raw);
    }

    if let Some(runtime_dir) = dirs::runtime_dir() {
        return runtime_dir.join("crimp.sock");
    }

    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "default".to_string());
    std::env::temp_dir().join(format!("crimp-{user}.sock"))
}

pub struct IpcServerHandle {
    socket_path: PathBuf,
    stop_flag: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl IpcServerHandle {
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn shutdown(mut self) {
        self.stop_internal();
    }

    fn stop_internal(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

impl Drop for IpcServerHandle {
    fn drop(&mut self) {
        self.stop_internal();
    }
}

pub fn start_ipc_server<F>(socket_path: PathBuf, handler: F) -> Result<IpcServerHandle, IpcError>
where
    F: Fn(IpcRequest) -> IpcResponse + Send + Sync + 'static,
{
    if socket_path.exists() {
        if UnixStream::connect(&socket_path).is_ok() {
            return Err(IpcError::AlreadyRunning(socket_path));
        }
        fs::remove_file(&socket_path).map_err(|source| IpcError::Io {
            path: socket_path.clone(),
            source,
        })?;
    }

    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent).map_err(|source| IpcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let listener = UnixListener::bind(&socket_path).map_err(|source| IpcError::Io {
        path: socket_path.clone(),
        source,
    })?;
    let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);
    let handler = Arc::new(handler);
    let path_for_log = socket_path.clone();

    let join_handle = thread::Builder::new()
        .name("crimp-ipc".into())
        .spawn(move || {
            debug!(socket = %path_for_log.display(), "IPC server naslouchá");
            for incoming in listener.incoming() {
                if stop_clone.load(Ordering::SeqCst) {
                    break;
                }
                match incoming {
                    Ok(stream) => {
                        let handler_ref = Arc::clone(&handler);
                        handle_client_connection(stream, &*handler_ref);
                    }
                    Err(err) => {
                        if !stop_clone.load(Ordering::SeqCst) {
                            warn!(error = %err, "Chyba při přijímání IPC spojení");
                        }
                    }
                }
            }
        })
        .map_err(|source| IpcError::Io {
            path: socket_path.clone(),
            source,
        })?;

    Ok(IpcServerHandle {
        socket_path,
        stop_flag,
        join_handle: Some(join_handle),
    })
}

fn handle_client_connection<F>(mut stream: UnixStream, handler: &F)
where
    F: Fn(IpcRequest) -> IpcResponse,
{
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(err) => {
            warn!(error = %err, "Selhalo klonování IPC streamu");
            return;
        }
    };
    let mut reader = BufReader::new(reader_stream);
    let mut line = String::new();

    let response = match reader.read_line(&mut line) {
        Ok(0) => return,
        Ok(_) => match serde_json::from_str::<IpcRequest>(line.trim()) {
            Ok(req) => handler(req),
            Err(err) => IpcResponse::Error {
                message: format!("Neplatný IPC požadavek: {err}"),
            },
        },
        Err(err) => IpcResponse::Error {
            message: format!("Chyba čtení IPC požadavku: {err}"),
        },
    };

    if let Ok(mut encoded) = serde_json::to_string(&response) {
        encoded.push('\n');
        let _ = stream.write_all(encoded.as_bytes());
        let _ = stream.flush();
    }
}

pub fn send_request(socket_path: &Path, req: &IpcRequest) -> Result<IpcResponse, IpcError> {
    let mut stream = UnixStream::connect(socket_path).map_err(|source| IpcError::Connect {
        path: socket_path.to_path_buf(),
        source,
    })?;
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let mut payload = serde_json::to_string(req)?;
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .map_err(|source| IpcError::Io {
            path: socket_path.to_path_buf(),
            source,
        })?;
    stream.flush().map_err(|source| IpcError::Io {
        path: socket_path.to_path_buf(),
        source,
    })?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|source| IpcError::Io {
            path: socket_path.to_path_buf(),
            source,
        })?;

    Ok(serde_json::from_str(response_line.trim())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn ipc_roundtrip_ping_and_pw_settings() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let sock = std::env::temp_dir().join(format!("crimp-proto-test-{nonce}.sock"));

        let server = start_ipc_server(sock.clone(), |req| match req {
            IpcRequest::Ping => IpcResponse::Pong {
                version: "0.2.0-test".into(),
            },
            IpcRequest::PwSettings => IpcResponse::PwSettings {
                data: PwRuntimeState {
                    clock_rate: Some("48000".into()),
                    clock_quantum: Some("256".into()),
                    ..Default::default()
                },
            },
            _ => IpcResponse::Error {
                message: "unexpected".into(),
            },
        })
        .expect("start ipc server");

        let pong = send_request(&sock, &IpcRequest::Ping).expect("ping");
        assert_eq!(
            pong,
            IpcResponse::Pong {
                version: "0.2.0-test".into()
            }
        );

        let limits_json = serde_json::to_string(&IpcRequest::SavePwLimits {
            min_quantum: Some(64),
            max_quantum: Some(2048),
            quantum_limit: None,
            allowed_rates: Some(vec![44100, 48000, 96000]),
        })
        .expect("serialize SavePwLimits");
        let parsed_limits: IpcRequest =
            serde_json::from_str(&limits_json).expect("deserialize SavePwLimits");
        assert!(matches!(
            parsed_limits,
            IpcRequest::SavePwLimits {
                min_quantum: Some(64),
                max_quantum: Some(2048),
                quantum_limit: None,
                ..
            }
        ));

        match send_request(&sock, &IpcRequest::PwSettings).expect("pw settings") {
            IpcResponse::PwSettings { data } => {
                assert_eq!(data.clock_quantum.as_deref(), Some("256"));
            }
            other => panic!("unexpected response: {other:?}"),
        }

        server.shutdown();
        assert!(!sock.exists());
    }
}
