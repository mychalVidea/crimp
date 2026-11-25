mod pw_thread;

use crimp_proto::config;
use crimp_proto::detect::{HostEnvironment, analyze_topology, apply_suggested_rules};
use crimp_proto::ipc::{
    DefaultDeviceKind, DspOverrideChange, IpcRequest, IpcResponse, MetadataTarget,
    resolve_socket_path, send_request, start_ipc_server,
};
use crimp_proto::update;
use pw_thread::{PwCommand, spawn_pw_thread};
use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliAction {
    Daemon {
        config_path: Option<PathBuf>,
        socket_path: Option<String>,
    },
    Ui,
    Doctor {
        socket_path: Option<String>,
    },
    Auto {
        socket_path: Option<String>,
        apply: bool,
    },
    Update {
        check_only: bool,
        force: bool,
    },
    Check {
        config_path: Option<PathBuf>,
    },
    Status {
        socket_path: Option<String>,
        json: bool,
    },
    Settings {
        socket_path: Option<String>,
        json: bool,
    },
    SetSetting {
        socket_path: Option<String>,
        key: String,
        value: Option<String>,
    },
    SetDefaultDevice {
        socket_path: Option<String>,
        kind: DefaultDeviceKind,
        node_name: Option<String>,
    },
    Graph {
        socket_path: Option<String>,
        json: bool,
    },
    BlockList {
        socket_path: Option<String>,
        json: bool,
    },
    Reload {
        socket_path: Option<String>,
    },
    SetQuantum {
        socket_path: Option<String>,
        change: DspOverrideChange,
    },
    SetRate {
        socket_path: Option<String>,
        change: DspOverrideChange,
    },
    Shutdown {
        socket_path: Option<String>,
    },
    Help,
    Version,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let action = match parse_cli(&args) {
        Ok(action) => action,
        Err(msg) => {
            eprintln!("Chyba argumentů: {msg}\n");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match action {
        CliAction::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        CliAction::Version => {
            println!("crimp {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        CliAction::Ui => launch_gui(),
        CliAction::Doctor { socket_path } => run_doctor(socket_path),
        CliAction::Auto { socket_path, apply } => run_auto(socket_path, apply),
        CliAction::Update { check_only, force } => run_update(check_only, force),
        CliAction::Check { config_path } => run_check(config_path),
        CliAction::Daemon {
            config_path,
            socket_path,
        } => {
            init_logging();
            run_daemon(config_path, socket_path)
        }
        other => run_client_command(other),
    }
}

fn launch_gui() -> ExitCode {
    let sibling = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("crimp-ui")));

    let binary = sibling
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("crimp-ui"));

    match Command::new(&binary).spawn() {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!(
                "Nepodařilo se spustit grafické rozhraní '{}': {err}",
                binary.display()
            );
            ExitCode::FAILURE
        }
    }
}

fn run_doctor(socket_override: Option<String>) -> ExitCode {
    let env_info = HostEnvironment::probe();
    println!("=== Crimp – Kontrola kompatibility & PipeWire prostředí ===");
    println!(
        "  Distribuce        : {} (id={}, rodina={:?})",
        env_info.os_pretty_name, env_info.os_id, env_info.family
    );
    println!("  Architektura      : {}", env_info.arch);
    println!("  Display server    : {}", env_info.display_session);
    println!(
        "  libpipewire-0.3   : {}",
        if env_info.libpipewire_present {
            "OK (nalezeno)"
        } else {
            "CHYBÍ"
        }
    );
    println!(
        "  pipewire-0 socket : {}",
        if env_info.pipewire_socket_present {
            "OK (běží)"
        } else {
            "NEBĚŽÍ / nenalezeno"
        }
    );
    println!(
        "  Deps pro build    : {}",
        env_info.family.install_dev_deps_hint()
    );

    let sock_path = resolve_socket_path(socket_override.as_deref());
    println!("  Crimp IPC socket  : {}", sock_path.display());

    match (
        send_request(&sock_path, &IpcRequest::PwSettings),
        send_request(&sock_path, &IpcRequest::Graph),
    ) {
        (Ok(IpcResponse::PwSettings { data: rt }), Ok(IpcResponse::Graph { data: graph })) => {
            let topo = analyze_topology(&graph);
            println!("\n=== Živý PipeWire & Auto-Detekce ===");
            println!(
                "  PipeWire Server   : {} ({})",
                rt.core_name.as_deref().unwrap_or("-"),
                rt.core_version.as_deref().unwrap_or("-")
            );
            if let Some((q, r, ms)) = rt.effective_latency() {
                println!("  Efektivní latence : {q} vzorků @ {r} Hz ({ms:.2} ms)");
            }
            println!(
                "  Doporučený sink   : {}",
                topo.preferred_sink.as_ref().map_or_else(
                    || "nenalezen".into(),
                    |d| format!("{} ({})", d.label, d.node_name)
                )
            );
            println!(
                "  Detekovaný mic    : {}",
                topo.preferred_mic.as_ref().map_or_else(
                    || "nenalezen".into(),
                    |d| format!("{} ({})", d.label, d.node_name)
                )
            );
            println!(
                "  HDMI monitory     : {} detekováno",
                topo.hdmi_monitors.len()
            );
            println!("  Virtuální sinky   : {}", topo.virtual_sinks.len());
        }
        _ => {
            println!(
                "\n  (Crimp daemon nyní neběží; pro živou analýzu audia ho spusťte přes `crimp daemon`.)"
            );
        }
    }

    if env_info.is_compatible_for_binaries() {
        println!("\nVerdikt: KOMPATIBILNÍ pro předkompilované balíčky Crimp.");
        ExitCode::SUCCESS
    } else {
        println!(
            "\nVerdikt: Vyžaduje doinstalovat `libpipewire-0.3` nebo spustit na Linux x86_64/aarch64."
        );
        ExitCode::FAILURE
    }
}

fn run_auto(socket_override: Option<String>, apply: bool) -> ExitCode {
    let sock_path = resolve_socket_path(socket_override.as_deref());
    let graph = match send_request(&sock_path, &IpcRequest::Graph) {
        Ok(IpcResponse::Graph { data }) => data,
        Ok(IpcResponse::Error { message }) => {
            eprintln!("Chyba daemona: {message}");
            return ExitCode::FAILURE;
        }
        Err(err) => {
            eprintln!("Daemon musí běžet pro auto-detekci PipeWire topologie: {err}");
            return ExitCode::FAILURE;
        }
        _ => return ExitCode::FAILURE,
    };

    let topo = analyze_topology(&graph);
    println!(
        "Detekovaný preferovaný výstup : {:?}",
        topo.preferred_sink.as_ref().map(|d| &d.node_name)
    );
    println!(
        "Detekovaný mikrofon           : {:?}",
        topo.preferred_mic.as_ref().map(|d| &d.node_name)
    );
    println!(
        "HDMI monitor porty            : {}",
        topo.hdmi_monitors.len()
    );

    if !apply {
        println!("\nPro automatické zapsání bezpečných doporučení spusť: `crimp auto --apply`");
        return ExitCode::SUCCESS;
    }

    let mut cfg = match send_request(&sock_path, &IpcRequest::GetConfig) {
        Ok(IpcResponse::Config { data }) => data,
        _ => {
            eprintln!("Nepodařilo se načíst aktuální konfiguraci z daemona");
            return ExitCode::FAILURE;
        }
    };

    let added = apply_suggested_rules(&mut cfg, &topo);
    if added == 0 {
        println!("Všechna doporučená pravidla jsou již v konfiguraci obsažena.");
        return ExitCode::SUCCESS;
    }

    match send_request(&sock_path, &IpcRequest::SaveConfig { config: cfg }) {
        Ok(IpcResponse::Ok { message }) => {
            println!("Přidáno {added} doporučení: {message}");
            ExitCode::SUCCESS
        }
        Ok(IpcResponse::Error { message }) => {
            eprintln!("Chyba uložení: {message}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("Chyba komunikace: {err}");
            ExitCode::FAILURE
        }
        _ => ExitCode::FAILURE,
    }
}

fn format_ver_commit(ver: &str, commit: Option<&str>) -> String {
    match commit {
        Some(c) if !c.is_empty() => format!("{ver} ({c})"),
        _ => ver.to_string(),
    }
}

fn run_update(check_only: bool, force: bool) -> ExitCode {
    println!("Kontroluji aktualizace Crimp na GitHubu...");
    match update::check_for_updates() {
        Ok(info) => {
            println!(
                "  Aktuální verze : {}\n  Dostupná verze : {} ({})",
                format_ver_commit(&info.current_version, info.current_commit.as_deref()),
                format_ver_commit(&info.latest_version, info.latest_commit.as_deref()),
                info.source_label
            );
            if !info.update_available && !force {
                println!("Crimp je aktuální.");
                return ExitCode::SUCCESS;
            }
            if check_only {
                if info.update_available {
                    println!(
                        "Je dostupná aktualizace: v{}",
                        format_ver_commit(&info.latest_version, info.latest_commit.as_deref())
                    );
                }
                return ExitCode::SUCCESS;
            }
        }
        Err(err) if !force || check_only => {
            eprintln!("Kontrola aktualizací selhala: {err}");
            return ExitCode::FAILURE;
        }
        Err(err) => {
            eprintln!("Varování při kontrole verze ({err}), vynucuji aktualizaci z GitHubu...");
        }
    }

    println!("Stahuji a instaluji nejnovější verzi...");
    match update::run_self_update() {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run_check(custom_config: Option<PathBuf>) -> ExitCode {
    let path = match custom_config {
        Some(p) => p,
        None => match config::default_config_path() {
            Ok(p) => p,
            Err(err) => {
                eprintln!("Chyba cesty ke konfiguraci: {err}");
                return ExitCode::FAILURE;
            }
        },
    };

    match config::load_existing(&path) {
        Ok(cfg) => {
            println!(
                "OK: {} (virtual_sink={}, deny={}, route={}, dsp_profile={})",
                path.display(),
                cfg.virtual_sink.len(),
                cfg.deny.len(),
                cfg.route.len(),
                cfg.dsp_profile.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("Neplatná konfigurace: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_daemon(custom_config: Option<PathBuf>, custom_socket: Option<String>) -> ExitCode {
    let config_path = match custom_config {
        Some(p) => p,
        None => match config::default_config_path() {
            Ok(p) => p,
            Err(err) => {
                error!(error = %err, "Nelze určit cestu ke konfiguraci");
                return ExitCode::FAILURE;
            }
        },
    };

    let cfg = match config::load_or_create_default(&config_path) {
        Ok(c) => c,
        Err(err) => {
            error!(path = %config_path.display(), error = %err, "Chyba při načítání konfigurace");
            return ExitCode::FAILURE;
        }
    };

    let ipc_enabled = cfg.ipc.enabled;
    let resolved_socket = if ipc_enabled {
        Some(resolve_socket_path(
            custom_socket.as_deref().or(cfg.ipc.socket_path.as_deref()),
        ))
    } else {
        None
    };

    info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %config_path.display(),
        virtual_sinks = cfg.virtual_sink.iter().filter(|s| s.enabled).count(),
        deny_rules = cfg.deny.iter().filter(|r| r.enabled).count(),
        route_rules = cfg.route.iter().filter(|r| r.enabled).count(),
        dsp_profiles = cfg.dsp_profile.iter().filter(|d| d.enabled).count(),
        "Spouštím Crimp"
    );

    let pw_handle = match spawn_pw_thread(
        config_path,
        cfg,
        resolved_socket.as_ref().map(|p| p.display().to_string()),
    ) {
        Ok(handle) => Arc::new(handle),
        Err(err) => {
            error!(error = %err, "Nepodařilo se spustit PipeWire vlákno");
            return ExitCode::FAILURE;
        }
    };

    let ipc_handle = if let Some(sock_path) = resolved_socket {
        let pw_for_ipc = Arc::clone(&pw_handle);
        match start_ipc_server(sock_path.clone(), move |req| match req {
            IpcRequest::Ping => IpcResponse::Pong {
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            IpcRequest::Status => match pw_for_ipc.request_status() {
                Ok(data) => IpcResponse::Status { data },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::Graph => match pw_for_ipc.request_graph() {
                Ok(data) => IpcResponse::Graph { data },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::PwSettings => match pw_for_ipc.request_pw_settings() {
                Ok(data) => IpcResponse::PwSettings { data },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::GetConfig => match pw_for_ipc.request_config() {
                Ok(data) => IpcResponse::Config { data },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::SaveConfig { config } => match pw_for_ipc.save_config(config) {
                Ok(message) => IpcResponse::Ok { message },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::DeniedHistory => match pw_for_ipc.request_denied_history() {
                Ok(entries) => IpcResponse::DeniedHistory { entries },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::Reload => match pw_for_ipc.reload_config() {
                Ok(message) => IpcResponse::Ok { message },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::SetDsp { quantum, rate } => {
                match pw_for_ipc.set_dsp_override(quantum, rate) {
                    Ok(target) => IpcResponse::Ok {
                        message: format!(
                            "DSP aktualizováno: quantum={:?}, rate={:?}, profil={:?}",
                            target.quantum, target.rate, target.active_profile
                        ),
                    },
                    Err(message) => IpcResponse::Error { message },
                }
            }
            IpcRequest::SetPwSetting { target, key, value } => {
                match pw_for_ipc.set_pw_setting(target, key, value) {
                    Ok(message) => IpcResponse::Ok { message },
                    Err(message) => IpcResponse::Error { message },
                }
            }
            IpcRequest::SetDefaultDevice { kind, node_name } => {
                match pw_for_ipc.set_default_device(kind, node_name) {
                    Ok(message) => IpcResponse::Ok { message },
                    Err(message) => IpcResponse::Error { message },
                }
            }
            IpcRequest::CreateLink {
                output_node_id,
                output_port_id,
                input_node_id,
                input_port_id,
                replace_input,
            } => match pw_for_ipc.create_manual_link(
                output_node_id,
                output_port_id,
                input_node_id,
                input_port_id,
                replace_input,
            ) {
                Ok(message) => IpcResponse::Ok { message },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::DestroyLink { link_id } => match pw_for_ipc.destroy_manual_link(link_id) {
                Ok(message) => IpcResponse::Ok { message },
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::Shutdown => {
                let _ = pw_for_ipc.send_command(PwCommand::Shutdown);
                IpcResponse::Ok {
                    message: "Crimp daemon se ukončuje".to_string(),
                }
            }
        }) {
            Ok(server) => {
                info!(socket = %server.socket_path().display(), "IPC server aktivní");
                Some(server)
            }
            Err(err) => {
                warn!(error = %err, "IPC server se nepodařilo spustit; pokračuji bez IPC");
                None
            }
        }
    } else {
        None
    };

    let pw_for_signal = Arc::clone(&pw_handle);
    if let Err(err) = ctrlc::set_handler(move || {
        info!("Zachycen ukončovací signál, zastavuji Crimp...");
        pw_for_signal.shutdown();
    }) {
        error!(error = %err, "Nelze zaregistrovat signal handler");
    }

    let join_result = pw_handle.join();
    drop(ipc_handle);

    match join_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!(error = %err, "Crimp skončil s chybou");
            ExitCode::FAILURE
        }
    }
}

fn run_client_command(action: CliAction) -> ExitCode {
    let (socket_override, req, json_output) = match action {
        CliAction::Status { socket_path, json } => (socket_path, IpcRequest::Status, json),
        CliAction::Settings { socket_path, json } => (socket_path, IpcRequest::PwSettings, json),
        CliAction::SetSetting {
            socket_path,
            key,
            value,
        } => (
            socket_path,
            IpcRequest::SetPwSetting {
                target: MetadataTarget::Settings,
                key,
                value,
            },
            false,
        ),
        CliAction::SetDefaultDevice {
            socket_path,
            kind,
            node_name,
        } => (
            socket_path,
            IpcRequest::SetDefaultDevice { kind, node_name },
            false,
        ),
        CliAction::Graph { socket_path, json } => (socket_path, IpcRequest::Graph, json),
        CliAction::BlockList { socket_path, json } => {
            (socket_path, IpcRequest::DeniedHistory, json)
        }
        CliAction::Reload { socket_path } => (socket_path, IpcRequest::Reload, false),
        CliAction::SetQuantum {
            socket_path,
            change,
        } => (
            socket_path,
            IpcRequest::SetDsp {
                quantum: change,
                rate: DspOverrideChange::Keep,
            },
            false,
        ),
        CliAction::SetRate {
            socket_path,
            change,
        } => (
            socket_path,
            IpcRequest::SetDsp {
                quantum: DspOverrideChange::Keep,
                rate: change,
            },
            false,
        ),
        CliAction::Shutdown { socket_path } => (socket_path, IpcRequest::Shutdown, false),
        _ => unreachable!(),
    };

    let sock_path = resolve_socket_path(socket_override.as_deref());
    let resp = match send_request(&sock_path, &req) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("Chyba komunikace s daemonem: {err}");
            return ExitCode::FAILURE;
        }
    };

    if json_output {
        match serde_json::to_string_pretty(&resp) {
            Ok(pretty) => println!("{pretty}"),
            Err(err) => {
                eprintln!("Chyba serializace JSON: {err}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    match resp {
        IpcResponse::Status { data } => {
            println!("Crimp v{}", data.version);
            println!("  Konfigurace : {}", data.config_path);
            println!(
                "  IPC socket  : {}",
                data.socket_path.as_deref().unwrap_or("vypnuto")
            );
            println!(
                "  Graf        : {} uzlů, {} portů, {} linků",
                data.node_count, data.port_count, data.link_count
            );
            println!(
                "  Pravidla    : {} deny, {} route, {} aktivních virtual_sink ({})",
                data.deny_rule_count,
                data.route_rule_count,
                data.virtual_sinks_active.len(),
                if data.virtual_sinks_active.is_empty() {
                    "žádné".to_string()
                } else {
                    data.virtual_sinks_active.join(", ")
                }
            );
            println!(
                "  DSP         : quantum={:?}, rate={:?}, profil={:?}",
                data.dsp_quantum, data.dsp_rate, data.dsp_active_profile
            );
            println!(
                "  Statistiky  : {} zablokovaných linků, {} aplikovaných rout",
                data.blocked_total, data.routes_applied_total
            );
            ExitCode::SUCCESS
        }
        IpcResponse::PwSettings { data } => {
            println!("=== PipeWire Server ===");
            println!(
                "  Core        : {} (verze {})",
                data.core_name.as_deref().unwrap_or("-"),
                data.core_version.as_deref().unwrap_or("-")
            );
            println!(
                "  Host / User : {}@{}",
                data.user_name.as_deref().unwrap_or("-"),
                data.host_name.as_deref().unwrap_or("-")
            );
            if let Some((q, r, ms)) = data.effective_latency() {
                println!("  Latence     : {q} vzorků @ {r} Hz ({ms:.2} ms)");
            }
            println!("\n=== PipeWire Clock & Settings ===");
            println!(
                "  clock.rate              : {}",
                data.clock_rate.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.force-rate        : {}",
                data.clock_force_rate.as_deref().unwrap_or("auto")
            );
            println!(
                "  clock.allowed-rates     : {}",
                data.clock_allowed_rates.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.quantum           : {}",
                data.clock_quantum.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.force-quantum     : {}",
                data.clock_force_quantum.as_deref().unwrap_or("auto")
            );
            println!(
                "  clock.min-quantum       : {}",
                data.clock_min_quantum.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.max-quantum       : {}",
                data.clock_max_quantum.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.quantum-limit     : {}",
                data.clock_quantum_limit.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.quantum-floor     : {}",
                data.clock_quantum_floor.as_deref().unwrap_or("-")
            );
            println!(
                "  clock.power-of-two      : {}",
                data.clock_power_of_two.as_deref().unwrap_or("-")
            );
            println!(
                "  log.level               : {}",
                data.log_level.as_deref().unwrap_or("-")
            );
            println!("\n=== Výchozí zařízení (default) ===");
            println!(
                "  default.audio.sink      : {}",
                data.default_audio_sink.as_deref().unwrap_or("-")
            );
            println!(
                "  default.audio.source    : {}",
                data.default_audio_source.as_deref().unwrap_or("-")
            );
            ExitCode::SUCCESS
        }
        IpcResponse::Graph { data } => {
            println!("=== Uzly ({}) ===", data.nodes.len());
            for node in data.nodes {
                let virt = if node.is_virtual_crimp {
                    " [VIRTUAL]"
                } else {
                    ""
                };
                println!(
                    "  #{:<4} {}{} [class={}] (out={}, in={})",
                    node.id,
                    node.label,
                    virt,
                    node.media_class.as_deref().unwrap_or("-"),
                    node.output_ports.len(),
                    node.input_ports.len()
                );
            }
            println!("\n=== Linky ({}) ===", data.links.len());
            for link in data.links {
                let tag = link
                    .enforced_by_rule
                    .as_deref()
                    .map(|r| format!(" [route: {r}]"))
                    .unwrap_or_default();
                println!(
                    "  #{:<4} {} (port {}) -> {} (port {}){}",
                    link.id,
                    link.output_label,
                    link.output_port_id,
                    link.input_label,
                    link.input_port_id,
                    tag
                );
            }
            ExitCode::SUCCESS
        }
        IpcResponse::DeniedHistory { entries } => {
            if entries.is_empty() {
                println!("Zatím nebyly zablokovány žádné linky.");
            } else {
                for e in entries {
                    println!(
                        "[{}] rule='{}' link#{} {} -> {}",
                        e.timestamp_epoch_ms,
                        e.rule_name,
                        e.link_id,
                        e.source_label,
                        e.target_label
                    );
                }
            }
            ExitCode::SUCCESS
        }
        IpcResponse::Config { data } => {
            println!("{data:#?}");
            ExitCode::SUCCESS
        }
        IpcResponse::Ok { message } => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        IpcResponse::Pong { version } => {
            println!("pong ({version})");
            ExitCode::SUCCESS
        }
        IpcResponse::Error { message } => {
            eprintln!("Chyba daemona: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_cli(args: &[String]) -> Result<CliAction, String> {
    let mut config_path: Option<PathBuf> = None;
    let mut socket_path: Option<String> = None;
    let mut json = false;
    let mut apply = false;
    let mut force = false;
    let mut check_only = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(CliAction::Help),
            "-V" | "--version" => return Ok(CliAction::Version),
            "--json" => json = true,
            "--apply" => apply = true,
            "--force" => force = true,
            "--check" => check_only = true,
            "-c" | "--config" => {
                i += 1;
                let val = args.get(i).ok_or("--config vyžaduje cestu")?;
                config_path = Some(PathBuf::from(val));
            }
            "-s" | "--socket" => {
                i += 1;
                let val = args.get(i).ok_or("--socket vyžaduje cestu")?;
                socket_path = Some(val.clone());
            }
            flag if flag.starts_with('-') => {
                return Err(format!("Neznámý přepínač '{flag}'"));
            }
            arg => positional.push(arg.to_string()),
        }
        i += 1;
    }

    let cmd = positional.first().map(String::as_str).unwrap_or("daemon");
    match cmd {
        "help" => Ok(CliAction::Help),
        "ui" | "gui" => Ok(CliAction::Ui),
        "doctor" => Ok(CliAction::Doctor { socket_path }),
        "auto" => Ok(CliAction::Auto { socket_path, apply }),
        "update" => Ok(CliAction::Update { check_only, force }),
        "daemon" => Ok(CliAction::Daemon {
            config_path,
            socket_path,
        }),
        "check" => Ok(CliAction::Check { config_path }),
        "status" => Ok(CliAction::Status { socket_path, json }),
        "settings" => Ok(CliAction::Settings { socket_path, json }),
        "set-setting" => {
            let key = positional
                .get(1)
                .ok_or("Příkaz `set-setting` vyžaduje klíč a hodnotu (např. clock.min-quantum 64)")?
                .clone();
            let raw_val = positional
                .get(2)
                .ok_or("Příkaz `set-setting` vyžaduje hodnotu (nebo `auto`)")?;
            let value = if matches!(
                raw_val.to_ascii_lowercase().as_str(),
                "auto" | "clear" | "reset"
            ) {
                None
            } else {
                Some(raw_val.clone())
            };
            Ok(CliAction::SetSetting {
                socket_path,
                key,
                value,
            })
        }
        "set-default-sink" => {
            let val = positional
                .get(1)
                .ok_or("Příkaz `set-default-sink` vyžaduje node.name nebo `auto`")?;
            let node_name = if matches!(
                val.to_ascii_lowercase().as_str(),
                "auto" | "clear" | "reset"
            ) {
                None
            } else {
                Some(val.clone())
            };
            Ok(CliAction::SetDefaultDevice {
                socket_path,
                kind: DefaultDeviceKind::Sink,
                node_name,
            })
        }
        "set-default-source" => {
            let val = positional
                .get(1)
                .ok_or("Příkaz `set-default-source` vyžaduje node.name nebo `auto`")?;
            let node_name = if matches!(
                val.to_ascii_lowercase().as_str(),
                "auto" | "clear" | "reset"
            ) {
                None
            } else {
                Some(val.clone())
            };
            Ok(CliAction::SetDefaultDevice {
                socket_path,
                kind: DefaultDeviceKind::Source,
                node_name,
            })
        }
        "graph" => Ok(CliAction::Graph { socket_path, json }),
        "block-list" => Ok(CliAction::BlockList { socket_path, json }),
        "reload" => Ok(CliAction::Reload { socket_path }),
        "shutdown" => Ok(CliAction::Shutdown { socket_path }),
        "set-quantum" => {
            let val = positional
                .get(1)
                .ok_or("Příkaz `set-quantum` vyžaduje hodnotu (číslo nebo `auto`)")?;
            Ok(CliAction::SetQuantum {
                socket_path,
                change: parse_dsp_override(val)?,
            })
        }
        "set-rate" => {
            let val = positional
                .get(1)
                .ok_or("Příkaz `set-rate` vyžaduje hodnotu (číslo nebo `auto`)")?;
            Ok(CliAction::SetRate {
                socket_path,
                change: parse_dsp_override(val)?,
            })
        }
        other => Err(format!("Neznámý příkaz '{other}'")),
    }
}

fn parse_dsp_override(raw: &str) -> Result<DspOverrideChange, String> {
    if matches!(
        raw.to_ascii_lowercase().as_str(),
        "auto" | "clear" | "reset"
    ) {
        return Ok(DspOverrideChange::Clear);
    }
    let val: u32 = raw.parse().map_err(|_| {
        format!("Neplatná číselná hodnota DSP '{raw}' (očekáváno číslo nebo 'auto')")
    })?;
    Ok(DspOverrideChange::Set(val))
}

fn print_usage() {
    println!(
        "crimp {} – PipeWire routing, virtual-sink, settings & blocklist daemon\n\n\
        POUŽITÍ:\n  \
          crimp [PŘÍKAZ] [VOLBY]\n\n\
        PŘÍKAZY:\n  \
          daemon                      Spustí PipeWire daemon (výchozí při spuštění bez příkazu)\n  \
          ui                          Otevře lehkou grafickou aplikaci (crimp-ui)\n  \
          doctor                      Ověří kompatibilitu OS, PipeWire, Wayland/X11 a detekci audia\n  \
          auto [--apply]              Automaticky detekuje výstup/mic/HDMI a navrhne (nebo zapíše) pravidla\n  \
          update [--check] [--force]  Zkontroluje a nainstaluje nejnovější verzi z GitHubu\n  \
          check                       Ověří syntaxi a validitu rules.toml bez PipeWire\n  \
          status                      Zobrazí stav běžícího daemona přes IPC\n  \
          settings                    Vypíše živá PipeWire Clock, Metadata a výchozí zařízení\n  \
          set-setting <KLÍČ> <VAL|auto>   Změní PipeWire settings metadata (např. clock.min-quantum 64)\n  \
          set-default-sink <NAME|auto>    Přepne defaultní audio výstup\n  \
          set-default-source <NAME|auto>  Přepne defaultní mikrofon / vstup\n  \
          graph                       Vypíše živé uzly, porty a linky v PipeWire grafu\n  \
          block-list                  Vypíše historii linků zablokovaných pravidly [[deny]]\n  \
          reload                      Okamžitě znovu načte konfiguraci pravidel za běhu\n  \
          set-quantum <N|auto>        Živě přepne clock.force-quantum (nebo vrátí auto)\n  \
          set-rate <N|auto>           Živě přepne clock.force-rate (nebo vrátí auto)\n  \
          shutdown                    Čistě ukončí běžícího daemona\n\n\
        VOLBY:\n  \
          -c, --config <CESTA>        Vlastní cesta k rules.toml\n  \
          -s, --socket <CESTA>        Vlastní cesta k Unix IPC socketu\n  \
              --json                  Výstup klientských příkazů ve formátu JSON\n  \
          -h, --help                  Zobrazí tuto nápovědu\n  \
          -V, --version               Vypíše verzi programu",
        env!("CARGO_PKG_VERSION")
    );
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
