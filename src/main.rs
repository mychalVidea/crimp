mod config;
mod graph;
mod pw_thread;
mod rules;

use std::sync::mpsc;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn main() {
    init_logging();

    let config_path = match config::default_config_path() {
        Ok(path) => path,
        Err(err) => {
            error!("Chyba cesty ke konfiguraci: {err}");
            std::process::exit(1);
        }
    };

    let cfg = match config::load_or_create_default(&config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("Nepodařilo se načíst konfiguraci: {err}");
            std::process::exit(1);
        }
    };

    info!(
        path = %config_path.display(),
        deny_rules = cfg.deny.len(),
        route_rules = cfg.route.len(),
        "Crimp inicializován"
    );

    let pw_handle = match pw_thread::spawn_pw_thread(cfg) {
        Ok(handle) => handle,
        Err(err) => {
            error!("Nepodařilo se spustit PipeWire vlákno: {err}");
            std::process::exit(1);
        }
    };

    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
    if let Err(err) = ctrlc::set_handler(move || {
        let _ = shutdown_tx.send(());
    }) {
        error!("Nepodařilo se zaregistrovat SIGINT/SIGTERM handler: {err}");
    }

    let _ = shutdown_rx.recv();
    info!("Přijat signál ukončení (SIGINT/SIGTERM), zastavuji Crimp...");
    pw_handle.shutdown();
    info!("Crimp čistě ukončen");
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("crimp=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
