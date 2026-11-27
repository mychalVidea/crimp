use crimp_proto::ipc::{IpcRequest, resolve_socket_path, send_request};
use ksni::blocking::TrayMethods;
use ksni::{Icon, MenuItem, ToolTip};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

fn ui_window_socket_path() -> PathBuf {
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("crimp-ui.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".into());
    std::env::temp_dir().join(format!("crimp-ui-{user}.sock"))
}

fn tray_socket_path() -> PathBuf {
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("crimp-tray.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".into());
    std::env::temp_dir().join(format!("crimp-tray-{user}.sock"))
}

fn send_line(sock: &PathBuf, msg: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(sock) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    stream.write_all(format!("{msg}\n").as_bytes()).is_ok()
}

/// Zkusí zaměřit již otevřené okno `crimp-ui`.
#[must_use]
pub fn try_focus_existing_window() -> bool {
    send_line(&ui_window_socket_path(), "show")
}

/// Spustí na pozadí lehký tray proces (`crimp-ui --tray`), pokud ještě neběží.
pub fn ensure_tray_running() {
    if send_line(&tray_socket_path(), "ping") {
        return;
    }
    let exe = crimp_proto::update::resolve_live_binary("crimp-ui");
    let _ = Command::new(exe).arg("--tray").spawn();
}

/// Restartuje běžící tray proces (např. po auto-updatu na novou binárku).
pub fn restart_tray_if_running() {
    if send_line(&tray_socket_path(), "quit") {
        thread::sleep(Duration::from_millis(120));
        let exe = crimp_proto::update::resolve_live_binary("crimp-ui");
        let _ = Command::new(exe).arg("--tray").spawn();
    }
}

/// Odstraní socket okna při ukončování/restartu.
pub fn cleanup_window_socket() {
    let _ = fs::remove_file(ui_window_socket_path());
}

/// Spustí posluchač okna pro příkazy `show` (zaměření) a `quit` (ukončení z tray menu).
pub struct WindowWakeGuard {
    sock_path: PathBuf,
    cleanup_on_drop: AtomicBool,
}

impl WindowWakeGuard {
    /// Při restartu UI zakáže starému oknu smazat `crimp-ui.sock` nové instance.
    pub fn disarm_cleanup(&self) {
        self.cleanup_on_drop.store(false, Ordering::SeqCst);
    }
}

impl Drop for WindowWakeGuard {
    fn drop(&mut self) {
        if self.cleanup_on_drop.load(Ordering::SeqCst) {
            let _ = fs::remove_file(&self.sock_path);
        }
    }
}

#[must_use]
pub fn spawn_window_wake_listener(
    ctx: egui::Context,
    quit_requested: Arc<AtomicBool>,
) -> WindowWakeGuard {
    let sock_path = ui_window_socket_path();
    let _ = fs::remove_file(&sock_path);
    if let Some(parent) = sock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(listener) = UnixListener::bind(&sock_path) {
        let _ = thread::Builder::new()
            .name("crimp-ui-wake".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok() {
                        match line.trim() {
                            "show" => {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                                ctx.request_repaint();
                            }
                            "quit" => {
                                quit_requested.store(true, Ordering::SeqCst);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                ctx.request_repaint();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            });
    }
    WindowWakeGuard {
        sock_path,
        cleanup_on_drop: AtomicBool::new(true),
    }
}

/// Samostatná lehká tray smyčka (`crimp-ui --tray`), bez OpenGL okna a bez Wayland `set_visible` bugu.
pub fn run_tray_service_blocking() {
    let sock_path = tray_socket_path();
    if send_line(&sock_path, "ping") {
        // Tray už běží.
        return;
    }
    let _ = fs::remove_file(&sock_path);
    if let Some(parent) = sock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let running = Arc::new(AtomicBool::new(true));
    let tray = CrimpTray {
        running: Arc::clone(&running),
        tray_sock: sock_path.clone(),
        icon: render_tray_icon_argb32(),
    };
    let _handle = tray.assume_sni_available(true).spawn();

    if let Ok(listener) = UnixListener::bind(&sock_path) {
        let _ = listener.set_nonblocking(true);
        while running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_ok() && line.trim() == "quit" {
                        let _ = send_line(&ui_window_socket_path(), "quit");
                        running.store(false, Ordering::SeqCst);
                        break;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(150));
                }
                Err(_) => break,
            }
        }
    } else {
        while running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(250));
        }
    }

    let _ = fs::remove_file(&sock_path);
}

fn open_or_focus_window() {
    if try_focus_existing_window() {
        return;
    }
    let exe = crimp_proto::update::resolve_live_binary("crimp-ui");
    let _ = Command::new(exe).arg("--window").spawn();
}

struct CrimpTray {
    running: Arc<AtomicBool>,
    tray_sock: PathBuf,
    icon: Vec<Icon>,
}

impl ksni::Tray for CrimpTray {
    fn id(&self) -> String {
        "crimp".to_string()
    }

    fn title(&self) -> String {
        "Crimp – PipeWire Patchbay".to_string()
    }

    fn icon_name(&self) -> String {
        "crimp".to_string()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Crimp Audio Patchbay".into(),
            description: "Kliknutím otevřeš Patchbay & PipeWire nastavení".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_or_focus_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Otevřít Crimp".into(),
                activate: Box::new(|_this: &mut Self| open_or_focus_window()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Skrýt okno do lišty".into(),
                activate: Box::new(|_this: &mut Self| {
                    let _ = send_line(&ui_window_socket_path(), "quit");
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Načíst pravidla (rules.toml)".into(),
                activate: Box::new(|_this: &mut Self| {
                    let sock = resolve_socket_path(None);
                    let _ = send_request(&sock, &IpcRequest::Reload);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Ukončit Crimp UI".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = send_line(&ui_window_socket_path(), "quit");
                    this.running.store(false, Ordering::SeqCst);
                    let _ = UnixStream::connect(&this.tray_sock);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Vygeneruje vestavěnou 32x32 ARGB32 ikonku (černý zaoblený čtverec s modrým a zeleným audio kabelem).
fn render_tray_icon_argb32() -> Vec<Icon> {
    let size = 32usize;
    let mut data = vec![0u8; size * size * 4];

    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let dx = (x as f32 - 15.5).abs();
            let dy = (y as f32 - 15.5).abs();
            let inside = dx <= 13.5
                && dy <= 13.5
                && !(dx > 10.5 && dy > 10.5 && (dx - 10.5).hypot(dy - 10.5) > 3.0);

            if !inside {
                continue;
            }

            let nx = x as f32 / 31.0;
            let ny = y as f32 / 31.0;

            let t = ((nx - 0.25) / 0.5).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            let blue_y = 0.32 + 0.36 * s;
            let green_y = 0.68 - 0.36 * s;

            let in_range = (0.20..=0.80).contains(&nx);
            let d_blue = if in_range { (ny - blue_y).abs() } else { 99.0 };
            let d_green = if in_range { (ny - green_y).abs() } else { 99.0 };

            let left_top = ((x as f32 - 7.5).hypot(y as f32 - 10.0)) <= 3.0;
            let left_bot = ((x as f32 - 7.5).hypot(y as f32 - 21.5)) <= 3.0;
            let right_top = ((x as f32 - 23.5).hypot(y as f32 - 10.0)) <= 3.0;
            let right_bot = ((x as f32 - 23.5).hypot(y as f32 - 21.5)) <= 3.0;

            let (r, g, b) = if left_top || right_bot || d_blue < 0.075 {
                (10, 103, 229)
            } else if left_bot || right_top || d_green < 0.065 {
                (33, 222, 0)
            } else {
                (16, 16, 20)
            };

            data[idx] = 255;
            data[idx + 1] = r;
            data[idx + 2] = g;
            data[idx + 3] = b;
        }
    }

    vec![Icon {
        width: size as i32,
        height: size as i32,
        data,
    }]
}
