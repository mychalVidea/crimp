use crimp_proto::config::{self, Config, PwSettingsConfig, VirtualSinkRule};
use crimp_proto::graph::{
    AudioGraph, DeniedRecord, GraphSnapshot, LinkInfo, NodeInfo, PortDirection, PortInfo,
    PwRuntimeState,
};
use crimp_proto::ipc::{DaemonStatus, DefaultDeviceKind, DspOverrideChange, MetadataTarget};
use crimp_proto::rules::{DenyAction, DspTarget, RouteAction, RuleEngine};
use pipewire::core::Listener as CoreListener;
use pipewire::link::Link;
use pipewire::metadata::{Metadata, MetadataListener};
use pipewire::node::Node;
use pipewire::properties::properties;
use pipewire::registry::{GlobalObject, RegistryRc};
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;
use pipewire::{self as pw, context::ContextRc, core::CoreRc, main_loop::MainLoopRc};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{debug, error, info, warn};

const MAX_DENIED_HISTORY: usize = 128;
const HOT_RELOAD_INTERVAL: Duration = Duration::from_secs(2);
const IN_FLIGHT_LINK_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Error)]
pub enum PwError {
    #[error("Selhala inicializace PipeWire main loopu")]
    MainLoopInit,

    #[error("Selhalo vytvoření PipeWire kontextu")]
    ContextInit,

    #[error("Nelze se připojit k PipeWire daemonu (běží pipewire.service?): {0}")]
    ConnectFailed(String),

    #[error("Nelze získat PipeWire registry: {0}")]
    RegistryFailed(String),

    #[error("PipeWire chyba: {0}")]
    PipeWire(#[from] pw::Error),
}

pub enum PwCommand {
    ReloadConfig(Option<mpsc::SyncSender<Result<String, String>>>),
    GetStatus(mpsc::SyncSender<DaemonStatus>),
    GetGraph(mpsc::SyncSender<GraphSnapshot>),
    GetPwSettings(mpsc::SyncSender<PwRuntimeState>),
    GetConfig(mpsc::SyncSender<Config>),
    SaveConfig {
        config: Config,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    GetDeniedHistory(mpsc::SyncSender<Vec<DeniedRecord>>),
    SetDspOverride {
        quantum: DspOverrideChange,
        rate: DspOverrideChange,
        reply: mpsc::SyncSender<Result<DspTarget, String>>,
    },
    SetPwSetting {
        target: MetadataTarget,
        key: String,
        value: Option<String>,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    SetDefaultDevice {
        kind: DefaultDeviceKind,
        node_name: Option<String>,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    CreateManualLink {
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
        replace_input: bool,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    DestroyManualLink {
        link_id: u32,
        reply: mpsc::SyncSender<Result<String, String>>,
    },
    Shutdown,
}

pub struct PwHandle {
    sender: pw::channel::Sender<PwCommand>,
    shutdown_flag: Arc<AtomicBool>,
    join_handle: Mutex<Option<JoinHandle<Result<(), PwError>>>>,
}

impl PwHandle {
    pub fn send_command(&self, cmd: PwCommand) -> Result<(), String> {
        if matches!(cmd, PwCommand::Shutdown) {
            self.shutdown_flag.store(true, Ordering::SeqCst);
        }
        self.sender
            .send(cmd)
            .map_err(|_| "PipeWire kanál již není aktivní".to_string())
    }

    pub fn request_status(&self) -> Result<DaemonStatus, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::GetStatus(tx))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout pro dotaz na stav: {e}"))
    }

    pub fn request_graph(&self) -> Result<GraphSnapshot, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::GetGraph(tx))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout pro čtení audio grafu: {e}"))
    }

    pub fn request_pw_settings(&self) -> Result<PwRuntimeState, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::GetPwSettings(tx))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout pro čtení PipeWire nastavení: {e}"))
    }

    pub fn request_config(&self) -> Result<Config, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::GetConfig(tx))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout pro načtení konfigurace: {e}"))
    }

    pub fn save_config(&self, config: Config) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::SaveConfig { config, reply: tx })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při ukládání konfigurace: {e}"))?
    }

    pub fn request_denied_history(&self) -> Result<Vec<DeniedRecord>, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::GetDeniedHistory(tx))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout pro čtení historie blokací: {e}"))
    }

    pub fn reload_config(&self) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::ReloadConfig(Some(tx)))?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při reloadu konfigurace: {e}"))?
    }

    pub fn set_dsp_override(
        &self,
        quantum: DspOverrideChange,
        rate: DspOverrideChange,
    ) -> Result<DspTarget, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::SetDspOverride {
            quantum,
            rate,
            reply: tx,
        })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při nastavování DSP: {e}"))?
    }

    pub fn set_pw_setting(
        &self,
        target: MetadataTarget,
        key: String,
        value: Option<String>,
    ) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::SetPwSetting {
            target,
            key,
            value,
            reply: tx,
        })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při úpravě PipeWire nastavení: {e}"))?
    }

    pub fn set_default_device(
        &self,
        kind: DefaultDeviceKind,
        node_name: Option<String>,
    ) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::SetDefaultDevice {
            kind,
            node_name,
            reply: tx,
        })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při změně výchozího zařízení: {e}"))?
    }

    pub fn create_manual_link(
        &self,
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
        replace_input: bool,
    ) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::CreateManualLink {
            output_node_id,
            output_port_id,
            input_node_id,
            input_port_id,
            replace_input,
            reply: tx,
        })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při vytváření linku: {e}"))?
    }

    pub fn destroy_manual_link(&self, link_id: u32) -> Result<String, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send_command(PwCommand::DestroyManualLink { link_id, reply: tx })?;
        rx.recv_timeout(Duration::from_secs(3))
            .map_err(|e| format!("Vypršel timeout při rozpojování linku: {e}"))?
    }

    pub fn shutdown(&self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        let _ = self.sender.send(PwCommand::Shutdown);
    }

    pub fn join(&self) -> Result<(), PwError> {
        let handle = self
            .join_handle
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());

        if let Some(handle) = handle {
            match handle.join() {
                Ok(res) => res,
                Err(_) => {
                    error!("PipeWire vlákno neočekávaně zpanikařilo");
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }
}

pub fn spawn_pw_thread(
    config_path: PathBuf,
    initial_config: Config,
    ipc_socket_label: Option<String>,
) -> std::io::Result<PwHandle> {
    let (sender, receiver) = pw::channel::channel::<PwCommand>();
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let flag_for_thread = Arc::clone(&shutdown_flag);

    let join_handle = thread::Builder::new()
        .name("crimp-pw".into())
        .spawn(move || {
            run_pipewire_loop(
                config_path,
                initial_config,
                ipc_socket_label,
                receiver,
                flag_for_thread,
            )
        })?;

    Ok(PwHandle {
        sender,
        shutdown_flag,
        join_handle: Mutex::new(Some(join_handle)),
    })
}

struct VirtualSinkEntry {
    spec: VirtualSinkRule,
    proxy: Node,
}

struct DaemonState {
    config_path: PathBuf,
    last_config_mtime: Option<SystemTime>,
    ipc_socket_label: Option<String>,
    core: CoreRc,
    registry: RegistryRc,
    runtime: Rc<RefCell<PwRuntimeState>>,
    graph: AudioGraph,
    engine: RuleEngine,
    link_factory: Option<String>,
    settings_metadata_id: Option<u32>,
    settings_metadata: Option<Metadata>,
    _settings_listener: Option<MetadataListener>,
    default_metadata_id: Option<u32>,
    default_metadata: Option<Metadata>,
    _default_listener: Option<MetadataListener>,
    virtual_sinks: BTreeMap<String, VirtualSinkEntry>,
    in_flight_links: HashMap<(u32, u32), Instant>,
    manual_quantum_override: Option<u32>,
    manual_rate_override: Option<u32>,
    last_applied_quantum: Option<u32>,
    last_applied_rate: Option<u32>,
    last_applied_pw_settings: PwSettingsConfig,
    denied_history: VecDeque<DeniedRecord>,
    blocked_total: u64,
    routes_applied_total: u64,
}

impl DaemonState {
    fn new(
        config_path: PathBuf,
        initial_config: Config,
        ipc_socket_label: Option<String>,
        core: CoreRc,
        registry: RegistryRc,
        runtime: Rc<RefCell<PwRuntimeState>>,
    ) -> Self {
        let mtime = fs::metadata(&config_path).and_then(|m| m.modified()).ok();

        Self {
            config_path,
            last_config_mtime: mtime,
            ipc_socket_label,
            core,
            registry,
            runtime,
            graph: AudioGraph::new(),
            engine: RuleEngine::new(initial_config),
            link_factory: None,
            settings_metadata_id: None,
            settings_metadata: None,
            _settings_listener: None,
            default_metadata_id: None,
            default_metadata: None,
            _default_listener: None,
            virtual_sinks: BTreeMap::new(),
            in_flight_links: HashMap::new(),
            manual_quantum_override: None,
            manual_rate_override: None,
            last_applied_quantum: None,
            last_applied_rate: None,
            last_applied_pw_settings: PwSettingsConfig::default(),
            denied_history: VecDeque::with_capacity(MAX_DENIED_HISTORY),
            blocked_total: 0,
            routes_applied_total: 0,
        }
    }

    fn on_global_added(&mut self, global: &GlobalObject<&DictRef>) {
        match global.type_ {
            ObjectType::Factory => {
                if let Some(props) = global.props
                    && props.get("factory.type.name") == Some(ObjectType::Link.to_str())
                    && let Some(name) = props.get("factory.name")
                {
                    self.link_factory = Some(name.to_string());
                }
            }
            ObjectType::Metadata => {
                if let Some(props) = global.props {
                    match props.get("metadata.name") {
                        Some("settings") => self.bind_settings_metadata(global),
                        Some("default") => self.bind_default_metadata(global),
                        _ => {}
                    }
                }
            }
            ObjectType::Node => {
                if let Some(props) = global.props {
                    let info = NodeInfo {
                        id: global.id,
                        node_name: props.get("node.name").map(str::to_string),
                        node_description: props.get("node.description").map(str::to_string),
                        application_name: props.get("application.name").map(str::to_string),
                        media_class: props.get("media.class").map(str::to_string),
                        node_latency: props.get("node.latency").map(str::to_string),
                        node_rate: props.get("node.rate").map(str::to_string),
                    };
                    debug!(
                        id = info.id,
                        label = %info.display_label(),
                        media_class = ?info.media_class,
                        "Node registrován"
                    );
                    self.graph.upsert_node(info);
                    self.enforce_all_links();
                    self.reconcile_routes();
                    self.reconcile_dsp();
                }
            }
            ObjectType::Port => {
                if let Some(props) = global.props
                    && let Some(node_id) = parse_u32(props.get("node.id"))
                {
                    let info = PortInfo {
                        id: global.id,
                        node_id,
                        port_name: props.get("port.name").map(str::to_string),
                        direction: PortDirection::from_pw_str(props.get("port.direction")),
                        channel: props.get("audio.channel").map(str::to_string),
                    };
                    self.graph.upsert_port(info);
                    self.enforce_all_links();
                    self.reconcile_routes();
                }
            }
            ObjectType::Link => {
                if let Some(props) = global.props
                    && let (
                        Some(output_node_id),
                        Some(output_port_id),
                        Some(input_node_id),
                        Some(input_port_id),
                    ) = (
                        parse_u32(props.get("link.output.node")),
                        parse_u32(props.get("link.output.port")),
                        parse_u32(props.get("link.input.node")),
                        parse_u32(props.get("link.input.port")),
                    )
                {
                    self.in_flight_links
                        .remove(&(output_port_id, input_port_id));
                    self.graph.upsert_link(LinkInfo {
                        id: global.id,
                        output_node_id,
                        output_port_id,
                        input_node_id,
                        input_port_id,
                    });
                    if !self.enforce_single_link(global.id) {
                        self.reconcile_routes();
                    }
                }
            }
            _ => {}
        }
    }

    fn bind_settings_metadata(&mut self, global: &GlobalObject<&DictRef>) {
        match self.registry.bind::<Metadata, _>(global) {
            Ok(metadata) => {
                info!(id = global.id, "Navázáno PipeWire metadata 'settings'");
                let runtime_ref = Rc::clone(&self.runtime);
                let listener = metadata
                    .add_listener_local()
                    .property(move |subject, key, _type, value| {
                        if subject != 0 {
                            return 0;
                        }
                        let mut rt = runtime_ref.borrow_mut();
                        match (key, value) {
                            (None, _) => {
                                rt.raw_settings_metadata.clear();
                                rt.clock_rate = None;
                                rt.clock_force_rate = None;
                                rt.clock_allowed_rates = None;
                                rt.clock_quantum = None;
                                rt.clock_force_quantum = None;
                                rt.clock_min_quantum = None;
                                rt.clock_max_quantum = None;
                                rt.clock_quantum_limit = None;
                                rt.clock_quantum_floor = None;
                                rt.clock_power_of_two = None;
                                rt.log_level = None;
                            }
                            (Some(k), val) => {
                                match val {
                                    Some(v) => {
                                        rt.raw_settings_metadata
                                            .insert(k.to_string(), v.to_string());
                                    }
                                    None => {
                                        rt.raw_settings_metadata.remove(k);
                                    }
                                }
                                let owned = val.map(str::to_string);
                                match k {
                                    "clock.rate" => rt.clock_rate = owned,
                                    "clock.force-rate" => rt.clock_force_rate = owned,
                                    "clock.allowed-rates" => rt.clock_allowed_rates = owned,
                                    "clock.quantum" => rt.clock_quantum = owned,
                                    "clock.force-quantum" => rt.clock_force_quantum = owned,
                                    "clock.min-quantum" => rt.clock_min_quantum = owned,
                                    "clock.max-quantum" => rt.clock_max_quantum = owned,
                                    "clock.quantum-limit" => rt.clock_quantum_limit = owned,
                                    "clock.quantum-floor" => rt.clock_quantum_floor = owned,
                                    "clock.power-of-two-quantum" => rt.clock_power_of_two = owned,
                                    "log.level" => rt.log_level = owned,
                                    _ => {}
                                }
                            }
                        }
                        0
                    })
                    .register();

                self.settings_metadata_id = Some(global.id);
                self.settings_metadata = Some(metadata);
                self._settings_listener = Some(listener);
                self.reconcile_pw_settings();
                self.reconcile_dsp();
            }
            Err(err) => warn!(error = %err, "Nelze navázat PipeWire metadata 'settings'"),
        }
    }

    fn bind_default_metadata(&mut self, global: &GlobalObject<&DictRef>) {
        match self.registry.bind::<Metadata, _>(global) {
            Ok(metadata) => {
                info!(id = global.id, "Navázáno PipeWire metadata 'default'");
                let runtime_ref = Rc::clone(&self.runtime);
                let listener = metadata
                    .add_listener_local()
                    .property(move |subject, key, _type, value| {
                        if subject != 0 {
                            return 0;
                        }
                        let mut rt = runtime_ref.borrow_mut();
                        match (key, value) {
                            (None, _) => {
                                rt.raw_default_metadata.clear();
                                rt.default_audio_sink = None;
                                rt.default_audio_source = None;
                                rt.default_configured_sink = None;
                                rt.default_configured_source = None;
                            }
                            (Some(k), val) => {
                                match val {
                                    Some(v) => {
                                        rt.raw_default_metadata
                                            .insert(k.to_string(), v.to_string());
                                    }
                                    None => {
                                        rt.raw_default_metadata.remove(k);
                                    }
                                }
                                let parsed = PwRuntimeState::extract_device_name(val);
                                match k {
                                    "default.audio.sink" => rt.default_audio_sink = parsed,
                                    "default.audio.source" => rt.default_audio_source = parsed,
                                    "default.configured.audio.sink" => {
                                        rt.default_configured_sink = parsed;
                                    }
                                    "default.configured.audio.source" => {
                                        rt.default_configured_source = parsed;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        0
                    })
                    .register();

                self.default_metadata_id = Some(global.id);
                self.default_metadata = Some(metadata);
                self._default_listener = Some(listener);
                self.reconcile_default_devices();
            }
            Err(err) => warn!(error = %err, "Nelze navázat PipeWire metadata 'default'"),
        }
    }

    fn on_global_removed(&mut self, id: u32) {
        if self.settings_metadata_id == Some(id) {
            self.settings_metadata_id = None;
            self.settings_metadata = None;
            self._settings_listener = None;
        }
        if self.default_metadata_id == Some(id) {
            self.default_metadata_id = None;
            self.default_metadata = None;
            self._default_listener = None;
        }
        if self.graph.remove_global(id) {
            debug!(id, "Globální PipeWire objekt odebrán");
            self.reconcile_routes();
            self.reconcile_dsp();
        }
    }

    fn enforce_single_link(&mut self, link_id: u32) -> bool {
        let Some(resolved) = self.graph.resolve_link(link_id) else {
            return false;
        };

        if let DenyAction::DestroyLink { link_id, rule_name } = self.engine.evaluate_link(&resolved)
        {
            let source_label = resolved.output_node.map_or_else(
                || format!("node#{}", resolved.link.output_node_id),
                NodeInfo::display_label,
            );
            let target_label = resolved.input_node.map_or_else(
                || format!("node#{}", resolved.link.input_node_id),
                NodeInfo::display_label,
            );

            match self.registry.destroy_global(link_id).into_result() {
                Ok(_) => {
                    self.blocked_total += 1;
                    self.record_denied(
                        rule_name.clone(),
                        link_id,
                        source_label.clone(),
                        target_label.clone(),
                    );
                    info!(
                        rule = %rule_name,
                        link_id,
                        source = %source_label,
                        target = %target_label,
                        "Link zablokován a rozpojen"
                    );
                    self.graph.remove_global(link_id);
                    return true;
                }
                Err(err) => warn!(link_id, error = ?err, "Selhalo zničení linku"),
            }
        }
        false
    }

    fn record_denied(
        &mut self,
        rule_name: String,
        link_id: u32,
        source_label: String,
        target_label: String,
    ) {
        if self.denied_history.len() >= MAX_DENIED_HISTORY {
            self.denied_history.pop_front();
        }
        let timestamp_epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        self.denied_history.push_back(DeniedRecord {
            timestamp_epoch_ms,
            rule_name,
            link_id,
            source_label,
            target_label,
        });
    }

    fn enforce_all_links(&mut self) {
        let link_ids: Vec<u32> = self.graph.links().map(|l| l.id).collect();
        for link_id in link_ids {
            self.enforce_single_link(link_id);
        }
    }

    fn reconcile_routes(&mut self) {
        let now = Instant::now();
        self.in_flight_links
            .retain(|_, created_at| now.duration_since(*created_at) < IN_FLIGHT_LINK_TTL);

        let actions = self.engine.plan_routes(&self.graph);
        let link_factory = self
            .link_factory
            .clone()
            .unwrap_or_else(|| "link-factory".to_string());

        for action in actions {
            match action {
                RouteAction::DestroyCompetingLink { link_id, rule_name } => {
                    match self.registry.destroy_global(link_id).into_result() {
                        Ok(_) => {
                            info!(
                                rule = %rule_name,
                                link_id,
                                "Odpojen konkurenční auto-link WirePlumberu"
                            );
                            self.graph.remove_global(link_id);
                        }
                        Err(err) => {
                            warn!(link_id, error = ?err, "Nelze odstranit konkurenční link")
                        }
                    }
                }
                RouteAction::CreateLink {
                    output_node_id,
                    output_port_id,
                    input_node_id,
                    input_port_id,
                    rule_name,
                } => {
                    if self
                        .in_flight_links
                        .contains_key(&(output_port_id, input_port_id))
                    {
                        continue;
                    }

                    let props = properties! {
                        "link.output.node" => output_node_id.to_string(),
                        "link.output.port" => output_port_id.to_string(),
                        "link.input.node" => input_node_id.to_string(),
                        "link.input.port" => input_port_id.to_string(),
                        "object.linger" => "1"
                    };

                    match self.core.create_object::<Link>(&link_factory, &props) {
                        Ok(_link_proxy) => {
                            self.in_flight_links
                                .insert((output_port_id, input_port_id), now);
                            self.routes_applied_total += 1;
                            info!(
                                rule = %rule_name,
                                output_node_id,
                                output_port_id,
                                input_node_id,
                                input_port_id,
                                "Vytvořen vynucený PipeWire link"
                            );
                        }
                        Err(err) => warn!(
                            rule = %rule_name,
                            error = %err,
                            "Selhalo vytvoření routovacího linku"
                        ),
                    }
                }
            }
        }
    }

    fn create_manual_link(
        &mut self,
        output_node_id: u32,
        output_port_id: u32,
        input_node_id: u32,
        input_port_id: u32,
        replace_input: bool,
    ) -> Result<String, String> {
        let out_node = self.graph.node(output_node_id);
        let out_port = self.graph.port(output_port_id);
        let in_node = self.graph.node(input_node_id);
        let in_port = self.graph.port(input_port_id);

        if out_node.is_none() || out_port.is_none() || in_node.is_none() || in_port.is_none() {
            return Err("Zadaný uzel nebo port v PipeWire grafu neexistuje".to_string());
        }

        if let Some(rule) = self
            .engine
            .find_deny_for_endpoints(out_node, out_port, in_node, in_port)
        {
            return Err(format!(
                "Link je zablokován pravidlem [[deny]] '{}'",
                rule.name
            ));
        }

        let mut replaced_count = 0usize;
        if replace_input {
            let conflicting_links = self.graph.conflicting_same_source_links(
                output_node_id,
                output_port_id,
                input_port_id,
            );

            for old_id in conflicting_links {
                if let Err(err) = self.registry.destroy_global(old_id).into_result() {
                    warn!(old_id, error = ?err, "Nepodařilo se odpojit předchozí link ze stejného zdrojového uzlu");
                } else {
                    self.graph.remove_global(old_id);
                    replaced_count += 1;
                }
            }
        }

        if self
            .graph
            .find_link_between_ports(output_port_id, input_port_id)
            .is_some()
        {
            return Ok(if replaced_count > 0 {
                format!("Předchozí linky ({replaced_count}) odpojeny, spojení zachováno")
            } else {
                "Link již existuje".to_string()
            });
        }

        let link_factory = self
            .link_factory
            .clone()
            .unwrap_or_else(|| "link-factory".to_string());

        let props = properties! {
            "link.output.node" => output_node_id.to_string(),
            "link.output.port" => output_port_id.to_string(),
            "link.input.node" => input_node_id.to_string(),
            "link.input.port" => input_port_id.to_string(),
            "object.linger" => "1"
        };

        self.core
            .create_object::<Link>(&link_factory, &props)
            .map_err(|e| format!("PipeWire odmítl vytvořit link: {e}"))?;

        self.in_flight_links
            .insert((output_port_id, input_port_id), Instant::now());
        info!(
            output_node_id,
            output_port_id,
            input_node_id,
            input_port_id,
            replaced_count,
            "Vytvořen ruční PipeWire link"
        );
        if replaced_count > 0 {
            Ok(format!(
                "Přepojeno na {output_node_id}:{output_port_id} -> {input_node_id}:{input_port_id} (předchozí kabel ze stejného zdroje odpojen)"
            ))
        } else {
            Ok(format!(
                "Vytvořen link {output_node_id}:{output_port_id} -> {input_node_id}:{input_port_id}"
            ))
        }
    }

    fn destroy_manual_link(&mut self, link_id: u32) -> Result<String, String> {
        if self.graph.link(link_id).is_none() {
            return Err(format!("Link #{link_id} v grafu neexistuje"));
        }
        self.registry
            .destroy_global(link_id)
            .into_result()
            .map_err(|e| format!("Selhalo rozpojení linku #{link_id}: {e:?}"))?;
        self.graph.remove_global(link_id);
        info!(link_id, "Link ručně rozpojen");
        Ok(format!("Link #{link_id} rozpojen"))
    }

    fn sync_virtual_sinks(&mut self) {
        let desired: BTreeMap<String, VirtualSinkRule> = self
            .engine
            .config()
            .virtual_sink
            .iter()
            .filter(|s| s.enabled)
            .map(|s| (s.name.clone(), s.clone()))
            .collect();

        let to_remove: Vec<String> = self
            .virtual_sinks
            .iter()
            .filter(|(name, entry)| desired.get(*name) != Some(&entry.spec))
            .map(|(name, _)| name.clone())
            .collect();

        for name in to_remove {
            if let Some(entry) = self.virtual_sinks.remove(&name) {
                info!(virtual_sink = %name, "Odstraňuji virtuální PipeWire sink");
                if let Err(err) = self.core.destroy_object(entry.proxy) {
                    debug!(virtual_sink = %name, error = %err, "Destroy virtuálního sinku vrátil chybu");
                }
            }
        }

        for (name, spec) in desired {
            if self.virtual_sinks.contains_key(&name) {
                continue;
            }

            let description = spec
                .description
                .clone()
                .unwrap_or_else(|| spec.name.clone());
            let mut props = properties! {
                "factory.name" => "support.null-audio-sink",
                "node.name" => spec.name.as_str(),
                "node.description" => description.as_str(),
                "media.class" => spec.media_class.as_str(),
                "audio.channels" => spec.channels.to_string(),
                "node.virtual" => "true",
                "monitor.channel-volumes" => "true"
            };
            if !spec.positions.is_empty() {
                props.insert(
                    "audio.position",
                    format!("[ {} ]", spec.positions.join(" ")),
                );
            }

            match self.core.create_object::<Node>("adapter", &props) {
                Ok(proxy) => {
                    info!(
                        virtual_sink = %name,
                        channels = spec.channels,
                        media_class = %spec.media_class,
                        "Vytvořen virtuální PipeWire sink"
                    );
                    self.virtual_sinks
                        .insert(name, VirtualSinkEntry { spec, proxy });
                }
                Err(err) => {
                    warn!(virtual_sink = %name, error = %err, "Nelze vytvořit virtuální sink")
                }
            }
        }
    }

    fn reconcile_pw_settings(&mut self) {
        let Some(metadata) = &self.settings_metadata else {
            return;
        };

        let target = self.engine.config().pw_settings.clone();
        let prev = &self.last_applied_pw_settings;

        apply_u32_setting(
            metadata,
            "clock.min-quantum",
            target.min_quantum,
            prev.min_quantum,
        );
        apply_u32_setting(
            metadata,
            "clock.max-quantum",
            target.max_quantum,
            prev.max_quantum,
        );
        apply_u32_setting(
            metadata,
            "clock.quantum-limit",
            target.quantum_limit,
            prev.quantum_limit,
        );
        apply_u32_setting(
            metadata,
            "clock.quantum-floor",
            target.quantum_floor,
            prev.quantum_floor,
        );

        if target.allowed_rates != prev.allowed_rates {
            match &target.allowed_rates {
                Some(rates) => {
                    let formatted = format!(
                        "[ {} ]",
                        rates
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    metadata.set_property(
                        0,
                        "clock.allowed-rates",
                        Some("Spa:String"),
                        Some(&formatted),
                    );
                    info!(allowed_rates = %formatted, "Nastaveno PipeWire clock.allowed-rates");
                }
                None if prev.allowed_rates.is_some() => {
                    metadata.set_property(0, "clock.allowed-rates", None, None);
                }
                None => {}
            }
        }

        if target.power_of_two_quantum != prev.power_of_two_quantum {
            match target.power_of_two_quantum {
                Some(b) => {
                    let s = if b { "true" } else { "false" };
                    metadata.set_property(
                        0,
                        "clock.power-of-two-quantum",
                        Some("Spa:String"),
                        Some(s),
                    );
                }
                None if prev.power_of_two_quantum.is_some() => {
                    metadata.set_property(0, "clock.power-of-two-quantum", None, None);
                }
                None => {}
            }
        }

        if target.log_level != prev.log_level {
            match target.log_level {
                Some(lvl) => {
                    let val = lvl.to_string();
                    metadata.set_property(0, "log.level", Some("Spa:String"), Some(&val));
                }
                None if prev.log_level.is_some() => {
                    metadata.set_property(0, "log.level", None, None);
                }
                None => {}
            }
        }

        self.last_applied_pw_settings = target;
    }

    fn reconcile_default_devices(&mut self) {
        let cfg = &self.engine.config().pw_settings;
        if let Some(sink) = cfg.default_audio_sink.clone() {
            let _ = self.set_default_device(DefaultDeviceKind::Sink, Some(sink));
        }
        if let Some(src) = cfg.default_audio_source.clone() {
            let _ = self.set_default_device(DefaultDeviceKind::Source, Some(src));
        }
    }

    fn set_pw_setting_live(
        &self,
        target: MetadataTarget,
        key: &str,
        value: Option<&str>,
    ) -> Result<String, String> {
        let key = key.trim();
        if !is_allowed_metadata_key(target, key) {
            return Err(format!(
                "Nepodporovaný nebo nepovolený klíč metadata '{key}'"
            ));
        }

        let meta = match target {
            MetadataTarget::Settings => self
                .settings_metadata
                .as_ref()
                .ok_or_else(|| "PipeWire metadata 'settings' není připojeno".to_string())?,
            MetadataTarget::Default => self
                .default_metadata
                .as_ref()
                .ok_or_else(|| "PipeWire metadata 'default' není připojeno".to_string())?,
        };

        let type_ = match (target, value) {
            (_, None) => None,
            (MetadataTarget::Default, Some(v)) if v.trim_start().starts_with('{') => {
                Some("Spa:String:JSON")
            }
            _ => Some("Spa:String"),
        };

        meta.set_property(0, key, type_, value);
        info!(?target, key, value = ?value, "Upraveno PipeWire metadata");
        Ok(match value {
            Some(v) => format!("{key} = {v}"),
            None => format!("{key} resetováno na výchozí"),
        })
    }

    fn set_default_device(
        &self,
        kind: DefaultDeviceKind,
        node_name: Option<String>,
    ) -> Result<String, String> {
        let meta = self
            .default_metadata
            .as_ref()
            .ok_or_else(|| "PipeWire metadata 'default' není připojeno".to_string())?;

        let (key, configured_key) = match kind {
            DefaultDeviceKind::Sink => ("default.audio.sink", "default.configured.audio.sink"),
            DefaultDeviceKind::Source => {
                ("default.audio.source", "default.configured.audio.source")
            }
        };

        match node_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(name) => {
                let json = serde_json::json!({ "name": name }).to_string();
                meta.set_property(0, configured_key, Some("Spa:String:JSON"), Some(&json));
                meta.set_property(0, key, Some("Spa:String:JSON"), Some(&json));
                info!(key, node_name = %name, "Nastaveno výchozí PipeWire zařízení");
                Ok(format!("{key} -> {name}"))
            }
            None => {
                meta.set_property(0, configured_key, None, None);
                meta.set_property(0, key, None, None);
                info!(key, "Obnoveno automatické výchozí PipeWire zařízení");
                Ok(format!("{key} resetováno na auto"))
            }
        }
    }

    fn reconcile_dsp(&mut self) {
        let Some(metadata) = &self.settings_metadata else {
            return;
        };

        let target = self.engine.evaluate_dsp(
            &self.graph,
            self.manual_quantum_override,
            self.manual_rate_override,
        );

        if target.quantum != self.last_applied_quantum {
            match target.quantum {
                Some(q) => {
                    let val = q.to_string();
                    metadata.set_property(0, "clock.force-quantum", Some("Spa:String"), Some(&val));
                    info!(
                        quantum = q,
                        profile = ?target.active_profile,
                        "Aplikováno PipeWire clock.force-quantum"
                    );
                    self.last_applied_quantum = Some(q);
                }
                None if self.last_applied_quantum.is_some() => {
                    metadata.set_property(0, "clock.force-quantum", None, None);
                    info!("Vráceno PipeWire clock.force-quantum na výchozí (auto)");
                    self.last_applied_quantum = None;
                }
                None => {}
            }
        }

        if target.rate != self.last_applied_rate {
            match target.rate {
                Some(r) => {
                    let val = r.to_string();
                    metadata.set_property(0, "clock.force-rate", Some("Spa:String"), Some(&val));
                    info!(
                        rate = r,
                        profile = ?target.active_profile,
                        "Aplikováno PipeWire clock.force-rate"
                    );
                    self.last_applied_rate = Some(r);
                }
                None if self.last_applied_rate.is_some() => {
                    metadata.set_property(0, "clock.force-rate", None, None);
                    info!("Vráceno PipeWire clock.force-rate na výchozí (auto)");
                    self.last_applied_rate = None;
                }
                None => {}
            }
        }
    }

    fn apply_config_and_sync(&mut self, cfg: Config, new_mtime: Option<SystemTime>) -> String {
        self.last_config_mtime = new_mtime;
        self.engine.update_config(cfg);
        self.sync_virtual_sinks();
        self.enforce_all_links();
        self.reconcile_routes();
        self.reconcile_pw_settings();
        self.reconcile_default_devices();
        self.reconcile_dsp();

        let c = self.engine.config();
        format!(
            "Konfigurace aktivní: {} virtual_sink, {} deny, {} route, {} dsp_profile",
            c.virtual_sink.len(),
            c.deny.len(),
            c.route.len(),
            c.dsp_profile.len(),
        )
    }

    fn reload_from_disk_if_changed(&mut self, force: bool) -> Result<String, String> {
        let current_mtime = fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();

        if !force && current_mtime == self.last_config_mtime {
            return Ok("Konfigurace beze změny".to_string());
        }

        match config::load_existing(&self.config_path) {
            Ok(cfg) => {
                let msg = self.apply_config_and_sync(cfg, current_mtime);
                info!("{msg}");
                Ok(msg)
            }
            Err(err) => {
                if current_mtime != self.last_config_mtime {
                    self.last_config_mtime = current_mtime;
                }
                error!(error = %err, "Hot-reload konfigurace selhal; ponechávám předchozí platná pravidla");
                Err(err.to_string())
            }
        }
    }

    fn save_and_apply_config(&mut self, cfg: Config) -> Result<String, String> {
        config::save_config(&self.config_path, &cfg).map_err(|e| e.to_string())?;
        let new_mtime = fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        let msg = self.apply_config_and_sync(cfg, new_mtime);
        info!(path = %self.config_path.display(), "Konfigurace uložena a aplikována");
        Ok(msg)
    }

    fn snapshot(&self) -> GraphSnapshot {
        let virtual_names: HashSet<String> = self.virtual_sinks.keys().cloned().collect();
        self.graph.snapshot_with_annotations(
            &virtual_names,
            |resolved| self.engine.matching_route_for_link(resolved),
            |node| self.engine.resolve_alias(node),
        )
    }

    fn status(&self) -> DaemonStatus {
        let (node_count, port_count, link_count) = self.graph.counts();
        let dsp = self.engine.evaluate_dsp(
            &self.graph,
            self.manual_quantum_override,
            self.manual_rate_override,
        );
        let cfg = self.engine.config();

        DaemonStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            config_path: self.config_path.display().to_string(),
            socket_path: self.ipc_socket_label.clone(),
            node_count,
            port_count,
            link_count,
            virtual_sinks_active: self.virtual_sinks.keys().cloned().collect(),
            deny_rule_count: cfg.deny.iter().filter(|r| r.enabled).count(),
            route_rule_count: cfg.route.iter().filter(|r| r.enabled).count(),
            dsp_active_profile: dsp.active_profile,
            dsp_quantum: dsp.quantum,
            dsp_rate: dsp.rate,
            manual_quantum_override: self.manual_quantum_override,
            manual_rate_override: self.manual_rate_override,
            blocked_total: self.blocked_total,
            routes_applied_total: self.routes_applied_total,
        }
    }

    fn cleanup_on_shutdown(&mut self) {
        if let Some(metadata) = &self.settings_metadata {
            if self.last_applied_quantum.is_some() {
                metadata.set_property(0, "clock.force-quantum", None, None);
                self.last_applied_quantum = None;
            }
            if self.last_applied_rate.is_some() {
                metadata.set_property(0, "clock.force-rate", None, None);
                self.last_applied_rate = None;
            }
        }

        let entries = std::mem::take(&mut self.virtual_sinks);
        for (name, entry) in entries {
            debug!(virtual_sink = %name, "Uvolňuji virtuální sink při vypnutí");
            let _ = self.core.destroy_object(entry.proxy);
        }
    }
}

fn apply_u32_setting(metadata: &Metadata, key: &str, desired: Option<u32>, prev: Option<u32>) {
    if desired == prev {
        return;
    }
    match desired {
        Some(v) => {
            let s = v.to_string();
            metadata.set_property(0, key, Some("Spa:String"), Some(&s));
            info!(key, value = v, "Aplikováno PipeWire nastavení");
        }
        None if prev.is_some() => {
            metadata.set_property(0, key, None, None);
            info!(key, "Vráceno PipeWire nastavení na výchozí");
        }
        None => {}
    }
}

fn is_allowed_metadata_key(target: MetadataTarget, key: &str) -> bool {
    match target {
        MetadataTarget::Settings => matches!(
            key,
            "clock.rate"
                | "clock.force-rate"
                | "clock.allowed-rates"
                | "clock.quantum"
                | "clock.force-quantum"
                | "clock.min-quantum"
                | "clock.max-quantum"
                | "clock.quantum-limit"
                | "clock.quantum-floor"
                | "clock.power-of-two-quantum"
                | "log.level"
        ),
        MetadataTarget::Default => matches!(
            key,
            "default.audio.sink"
                | "default.audio.source"
                | "default.configured.audio.sink"
                | "default.configured.audio.source"
        ),
    }
}

struct PersistentDspOverrides {
    quantum: Option<u32>,
    rate: Option<u32>,
}

fn run_pipewire_loop(
    config_path: PathBuf,
    initial_config: Config,
    ipc_socket_label: Option<String>,
    mut receiver: pw::channel::Receiver<PwCommand>,
    shutdown_flag: Arc<AtomicBool>,
) -> Result<(), PwError> {
    pw::init();

    let mut current_config = initial_config;
    let mut overrides = PersistentDspOverrides {
        quantum: None,
        rate: None,
    };

    while !shutdown_flag.load(Ordering::SeqCst) {
        if let Ok(fresh) = config::load_existing(&config_path) {
            current_config = fresh;
        }

        match run_single_pipewire_session(
            &config_path,
            current_config.clone(),
            ipc_socket_label.as_deref(),
            receiver,
            &shutdown_flag,
            &mut overrides,
        ) {
            Ok((returned_rx, should_reconnect)) => {
                receiver = returned_rx;
                if !should_reconnect || shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }
                warn!("PipeWire spojení přerušeno, zkusím automatické znovupřipojení...");
                sleep_interruptible(Duration::from_millis(1500), &shutdown_flag);
            }
            Err((returned_rx, err)) => {
                receiver = returned_rx;
                if shutdown_flag.load(Ordering::SeqCst) {
                    break;
                }
                warn!(error = %err, "PipeWire není připraven, opakuji připojení za 2 s...");
                sleep_interruptible(Duration::from_secs(2), &shutdown_flag);
            }
        }
    }

    Ok(())
}

fn sleep_interruptible(total: Duration, shutdown_flag: &AtomicBool) {
    let step = Duration::from_millis(200);
    let mut elapsed = Duration::ZERO;
    while elapsed < total && !shutdown_flag.load(Ordering::SeqCst) {
        thread::sleep(step);
        elapsed += step;
    }
}

fn run_single_pipewire_session(
    config_path: &PathBuf,
    initial_config: Config,
    ipc_socket_label: Option<&str>,
    receiver: pw::channel::Receiver<PwCommand>,
    shutdown_flag: &Arc<AtomicBool>,
    overrides: &mut PersistentDspOverrides,
) -> Result<(pw::channel::Receiver<PwCommand>, bool), (pw::channel::Receiver<PwCommand>, PwError)> {
    let mainloop = match MainLoopRc::new(None) {
        Ok(m) => m,
        Err(_) => return Err((receiver, PwError::MainLoopInit)),
    };
    let context = match ContextRc::new(&mainloop, None) {
        Ok(c) => c,
        Err(_) => return Err((receiver, PwError::ContextInit)),
    };
    let core = match context.connect_rc(None) {
        Ok(c) => c,
        Err(e) => return Err((receiver, PwError::ConnectFailed(e.to_string()))),
    };
    let registry = match core.get_registry_rc() {
        Ok(r) => r,
        Err(e) => return Err((receiver, PwError::RegistryFailed(e.to_string()))),
    };

    let runtime = Rc::new(RefCell::new(PwRuntimeState::default()));
    let disconnect_flag = Rc::new(std::cell::Cell::new(false));

    let runtime_core = Rc::clone(&runtime);
    let disconnect_core = Rc::clone(&disconnect_flag);
    let mainloop_err = mainloop.clone();
    let _core_listener: CoreListener = core
        .add_listener_local()
        .info(move |info| {
            let mut rt = runtime_core.borrow_mut();
            rt.core_name = Some(info.name().to_string());
            rt.core_version = Some(info.version().to_string());
            rt.user_name = Some(info.user_name().to_string());
            rt.host_name = Some(info.host_name().to_string());
            if let Some(props) = info.props() {
                rt.raw_core_props = props
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
            }
        })
        .error(move |id, _seq, res, message| {
            if id == pw::core::PW_ID_CORE {
                warn!(res, message, "PipeWire Core ohlásil fatální chybu spojení");
                disconnect_core.set(true);
                mainloop_err.quit();
            }
        })
        .register();

    let mut initial_state = DaemonState::new(
        config_path.clone(),
        initial_config,
        ipc_socket_label.map(str::to_string),
        core,
        registry.clone(),
        Rc::clone(&runtime),
    );
    initial_state.manual_quantum_override = overrides.quantum;
    initial_state.manual_rate_override = overrides.rate;

    let state = Rc::new(RefCell::new(initial_state));

    state.borrow_mut().sync_virtual_sinks();

    let state_add = Rc::clone(&state);
    let state_remove = Rc::clone(&state);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            state_add.borrow_mut().on_global_added(global);
        })
        .global_remove(move |id| {
            state_remove.borrow_mut().on_global_removed(id);
        })
        .register();

    let state_cmd = Rc::clone(&state);
    let mainloop_cmd = mainloop.clone();
    let shutdown_cmd = Arc::clone(shutdown_flag);
    let cmd_listener = receiver.attach(mainloop.loop_(), move |cmd| match cmd {
        PwCommand::ReloadConfig(reply) => {
            let res = state_cmd.borrow_mut().reload_from_disk_if_changed(true);
            if let Some(tx) = reply {
                let _ = tx.send(res);
            }
        }
        PwCommand::GetStatus(tx) => {
            let status = state_cmd.borrow().status();
            let _ = tx.send(status);
        }
        PwCommand::GetGraph(tx) => {
            let snapshot = state_cmd.borrow().snapshot();
            let _ = tx.send(snapshot);
        }
        PwCommand::GetPwSettings(tx) => {
            let rt = state_cmd.borrow().runtime.borrow().clone();
            let _ = tx.send(rt);
        }
        PwCommand::GetConfig(tx) => {
            let cfg = state_cmd.borrow().engine.config().clone();
            let _ = tx.send(cfg);
        }
        PwCommand::SaveConfig { config, reply } => {
            let res = state_cmd.borrow_mut().save_and_apply_config(config);
            let _ = reply.send(res);
        }
        PwCommand::GetDeniedHistory(tx) => {
            let entries: Vec<DeniedRecord> =
                state_cmd.borrow().denied_history.iter().cloned().collect();
            let _ = tx.send(entries);
        }
        PwCommand::SetDspOverride {
            quantum,
            rate,
            reply,
        } => {
            let mut st = state_cmd.borrow_mut();
            match quantum {
                DspOverrideChange::Keep => {}
                DspOverrideChange::Clear => st.manual_quantum_override = None,
                DspOverrideChange::Set(0) => {
                    let _ = reply.send(Err("Quantum nesmí být 0".to_string()));
                    return;
                }
                DspOverrideChange::Set(q) => st.manual_quantum_override = Some(q),
            }
            match rate {
                DspOverrideChange::Keep => {}
                DspOverrideChange::Clear => st.manual_rate_override = None,
                DspOverrideChange::Set(r) if r < 8_000 => {
                    let _ = reply.send(Err("Sample rate musí být alespoň 8000 Hz".to_string()));
                    return;
                }
                DspOverrideChange::Set(r) => st.manual_rate_override = Some(r),
            }
            st.reconcile_dsp();
            let target = st.engine.evaluate_dsp(
                &st.graph,
                st.manual_quantum_override,
                st.manual_rate_override,
            );
            let _ = reply.send(Ok(target));
        }
        PwCommand::SetPwSetting {
            target,
            key,
            value,
            reply,
        } => {
            let res = state_cmd
                .borrow()
                .set_pw_setting_live(target, &key, value.as_deref());
            let _ = reply.send(res);
        }
        PwCommand::SetDefaultDevice {
            kind,
            node_name,
            reply,
        } => {
            let res = state_cmd.borrow().set_default_device(kind, node_name);
            let _ = reply.send(res);
        }
        PwCommand::CreateManualLink {
            output_node_id,
            output_port_id,
            input_node_id,
            input_port_id,
            replace_input,
            reply,
        } => {
            let res = state_cmd.borrow_mut().create_manual_link(
                output_node_id,
                output_port_id,
                input_node_id,
                input_port_id,
                replace_input,
            );
            let _ = reply.send(res);
        }
        PwCommand::DestroyManualLink { link_id, reply } => {
            let res = state_cmd.borrow_mut().destroy_manual_link(link_id);
            let _ = reply.send(res);
        }
        PwCommand::Shutdown => {
            info!("Ukončuji PipeWire smyčku...");
            shutdown_cmd.store(true, Ordering::SeqCst);
            state_cmd.borrow_mut().cleanup_on_shutdown();
            mainloop_cmd.quit();
        }
    });

    let state_timer = Rc::clone(&state);
    let mainloop_timer = mainloop.clone();
    let shutdown_timer = Arc::clone(shutdown_flag);
    let hot_reload_timer = mainloop.loop_().add_timer(move |_| {
        if shutdown_timer.load(Ordering::SeqCst) {
            state_timer.borrow_mut().cleanup_on_shutdown();
            mainloop_timer.quit();
            return;
        }
        let _ = state_timer.borrow_mut().reload_from_disk_if_changed(false);
    });
    let _ = hot_reload_timer.update_timer(Some(HOT_RELOAD_INTERVAL), Some(HOT_RELOAD_INTERVAL));

    info!("PipeWire smyčka spuštěna");
    mainloop.run();
    info!("PipeWire smyčka ukončena");

    {
        let st = state.borrow();
        overrides.quantum = st.manual_quantum_override;
        overrides.rate = st.manual_rate_override;
    }

    drop(hot_reload_timer);
    let returned_rx = cmd_listener.deattach();
    let should_reconnect = disconnect_flag.get() && !shutdown_flag.load(Ordering::SeqCst);
    Ok((returned_rx, should_reconnect))
}

fn parse_u32(val: Option<&str>) -> Option<u32> {
    val.and_then(|s| s.trim().parse::<u32>().ok())
}
