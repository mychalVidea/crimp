use std::cell::RefCell;
use std::rc::Rc;
use std::thread::{self, JoinHandle};

use pipewire::context::ContextRc;
use pipewire::main_loop::MainLoopRc;
use pipewire::registry::RegistryRc;
use pipewire::types::ObjectType;
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::graph::{AudioGraph, LinkInfo, NodeInfo, PortInfo};
use crate::rules::{RuleAction, RuleEngine};

#[derive(Debug, Error)]
pub enum PwThreadError {
    #[error("Nepodařilo se spustit PipeWire vlákno: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("Nepodařilo se inicializovat PipeWire: {0}")]
    PipeWire(#[from] pipewire::Error),
}

#[derive(Debug, Clone)]
pub enum PwCommand {
    ReloadConfig(Config),
    Shutdown,
}

pub struct PwHandle {
    sender: pipewire::channel::Sender<PwCommand>,
    join_handle: Option<JoinHandle<()>>,
}

impl PwHandle {
    pub fn send(&self, cmd: PwCommand) -> Result<(), PwCommand> {
        self.sender.send(cmd)
    }

    pub fn shutdown(mut self) {
        let _ = self.sender.send(PwCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            if let Err(err) = handle.join() {
                error!("PipeWire vlákno skončilo panikou: {:?}", err);
            }
        }
    }
}

struct DaemonState {
    graph: AudioGraph,
    rules: RuleEngine,
}

pub fn spawn_pw_thread(initial_config: Config) -> Result<PwHandle, PwThreadError> {
    let (sender, receiver) = pipewire::channel::channel::<PwCommand>();

    let join_handle = thread::Builder::new()
        .name("crimp-pw".to_string())
        .spawn(move || {
            if let Err(err) = run_pipewire_loop(initial_config, receiver) {
                error!("PipeWire event loop selhal: {err}");
            }
        })?;

    Ok(PwHandle {
        sender,
        join_handle: Some(join_handle),
    })
}

fn run_pipewire_loop(
    initial_config: Config,
    receiver: pipewire::channel::Receiver<PwCommand>,
) -> Result<(), pipewire::Error> {
    pipewire::init();

    let mainloop = MainLoopRc::new(None)?;
    let context = ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry: RegistryRc = core.get_registry_rc()?;

    let state = Rc::new(RefCell::new(DaemonState {
        graph: AudioGraph::new(),
        rules: RuleEngine::new(initial_config),
    }));

    // Kanál pro řídicí zprávy z hlavního vlákna (reload configu, graceful shutdown)
    let _cmd_receiver = receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let state = Rc::clone(&state);
        let registry = registry.clone();
        move |cmd| match cmd {
            PwCommand::ReloadConfig(new_cfg) => {
                info!(
                    deny = new_cfg.deny.len(),
                    route = new_cfg.route.len(),
                    "Načítám aktualizovaná pravidla v PipeWire vlákně"
                );
                let mut st = state.borrow_mut();
                st.rules.update_config(new_cfg);
                enforce_all_links(&st.graph, &st.rules, &registry);
            }
            PwCommand::Shutdown => {
                info!("Přijat požadavek na ukončení PipeWire smyčky");
                mainloop.quit();
            }
        }
    });

    // Poslech změn globálního PipeWire grafu
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let state = Rc::clone(&state);
            let registry = registry.clone();
            move |global| {
                let Some(props) = global.props else {
                    return;
                };

                let mut st = state.borrow_mut();
                match global.type_ {
                    ObjectType::Node => {
                        let node = NodeInfo {
                            id: global.id,
                            node_name: props.get("node.name").map(ToString::to_string),
                            app_name: props.get("application.name").map(ToString::to_string),
                            media_class: props.get("media.class").map(ToString::to_string),
                        };
                        debug!(
                            id = node.id,
                            node_name = ?node.node_name,
                            app_name = ?node.app_name,
                            media_class = ?node.media_class,
                            "Registrován Node"
                        );
                        let planned_routes = st.rules.evaluate_node_routes(&node, &st.graph);
                        for action in planned_routes {
                            if let RuleAction::EnsureRoute {
                                source_node_id,
                                target_node_id,
                                rule_name,
                            } = action
                            {
                                debug!(
                                    rule = %rule_name,
                                    source_node_id,
                                    target_node_id,
                                    "Detekována shoda route pravidla (připraveno pro automatické přepojení)"
                                );
                            }
                        }
                        st.graph.upsert_node(node);
                    }
                    ObjectType::Port => {
                        let port = PortInfo {
                            id: global.id,
                            node_id: props.get("node.id").and_then(|v| v.parse::<u32>().ok()),
                            port_name: props.get("port.name").map(ToString::to_string),
                            direction: props.get("port.direction").map(ToString::to_string),
                        };
                        debug!(
                            id = port.id,
                            node_id = ?port.node_id,
                            port_name = ?port.port_name,
                            "Registrován Port"
                        );
                        st.graph.upsert_port(port);
                    }
                    ObjectType::Link => {
                        let (
                            Some(output_node_id),
                            Some(output_port_id),
                            Some(input_node_id),
                            Some(input_port_id),
                        ) = (
                            props.get("link.output.node").and_then(|v| v.parse::<u32>().ok()),
                            props.get("link.output.port").and_then(|v| v.parse::<u32>().ok()),
                            props.get("link.input.node").and_then(|v| v.parse::<u32>().ok()),
                            props.get("link.input.port").and_then(|v| v.parse::<u32>().ok()),
                        )
                        else {
                            return;
                        };

                        let link = LinkInfo {
                            id: global.id,
                            output_node_id,
                            output_port_id,
                            input_node_id,
                            input_port_id,
                        };
                        st.graph.upsert_link(link);
                        evaluate_and_enforce_link(&st.graph, &st.rules, &registry, global.id);
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let state = Rc::clone(&state);
            move |id| {
                debug!(id, "Odstraněn PipeWire global");
                state.borrow_mut().graph.remove_global(id);
            }
        })
        .register();

    info!("Crimp PipeWire monitor spuštěn");
    mainloop.run();
    Ok(())
}

fn evaluate_and_enforce_link(
    graph: &AudioGraph,
    rules: &RuleEngine,
    registry: &RegistryRc,
    link_id: u32,
) {
    let Some(resolved) = graph.resolve_link(link_id) else {
        return;
    };

    if let Some(RuleAction::DestroyLink {
        link_id,
        rule_name,
        reason,
    }) = rules.evaluate_link(&resolved)
    {
        match registry.destroy_global(link_id).into_result() {
            Ok(_) => info!(link_id, rule = %rule_name, reason = %reason, "Blokovaný link okamžitě rozpojen"),
            Err(err) => warn!(
                link_id,
                rule = %rule_name,
                error = ?err,
                "Nepodařilo se zničit blokovaný PipeWire link"
            ),
        }
    }
}

fn enforce_all_links(graph: &AudioGraph, rules: &RuleEngine, registry: &RegistryRc) {
    for link_id in graph.all_link_ids() {
        evaluate_and_enforce_link(graph, rules, registry, link_id);
    }
}
