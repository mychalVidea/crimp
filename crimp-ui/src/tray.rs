use ksni::blocking::TrayMethods;
use ksni::{Icon, MenuItem, ToolTip};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWindow,
    HideWindow,
    ToggleWindow,
    ReloadRules,
    QuitApp,
}

pub struct TrayController {
    rx: Receiver<TrayCommand>,
    _tray_handle: Option<ksni::blocking::Handle<CrimpTray>>,
    ui_sock_path: PathBuf,
}

impl TrayController {
    #[must_use]
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel::<TrayCommand>();
        let ui_sock_path = ui_socket_path();

        spawn_ui_wake_listener(ui_sock_path.clone(), tx.clone(), ctx.clone());

        let tray = CrimpTray {
            tx,
            ctx,
            icon: render_tray_icon_argb32(),
        };
        let tray_handle = tray.assume_sni_available(true).spawn().ok();

        Self {
            rx,
            _tray_handle: tray_handle,
            ui_sock_path,
        }
    }

    pub fn try_recv(&self) -> Option<TrayCommand> {
        self.rx.try_recv().ok()
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.ui_sock_path);
    }
}

/// Zjistí, zda už běží instance `crimp-ui` (např. minimalizovaná v trayi).
/// Pokud ano, požádá ji o zobrazení okna a vrátí `true`.
#[must_use]
pub fn try_focus_existing_instance() -> bool {
    let sock = ui_socket_path();
    let Ok(mut stream) = UnixStream::connect(&sock) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    stream.write_all(b"show\n").is_ok()
}

fn ui_socket_path() -> PathBuf {
    if let Some(runtime) = dirs::runtime_dir() {
        return runtime.join("crimp-ui.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".into());
    std::env::temp_dir().join(format!("crimp-ui-{user}.sock"))
}

fn spawn_ui_wake_listener(sock_path: PathBuf, tx: Sender<TrayCommand>, ctx: egui::Context) {
    if sock_path.exists() {
        let _ = fs::remove_file(&sock_path);
    }
    if let Some(parent) = sock_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(listener) = UnixListener::bind(&sock_path) else {
        return;
    };

    let _ = thread::Builder::new()
        .name("crimp-ui-wake".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() && line.trim() == "show" {
                    let _ = tx.send(TrayCommand::ShowWindow);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
            }
        });
}

struct CrimpTray {
    tx: Sender<TrayCommand>,
    ctx: egui::Context,
    icon: Vec<Icon>,
}

impl CrimpTray {
    fn emit(&self, cmd: TrayCommand) {
        let _ = self.tx.send(cmd);
        match cmd {
            TrayCommand::ShowWindow => {
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            TrayCommand::HideWindow => {
                self.ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            TrayCommand::ToggleWindow | TrayCommand::QuitApp | TrayCommand::ReloadRules => {}
        }
        self.ctx.request_repaint();
    }
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
            description: "Kliknutím otevřeš nebo skryješ okno".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.emit(TrayCommand::ToggleWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Otevřít Patchbay".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayCommand::ShowWindow)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Skrýt do lišty".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayCommand::HideWindow)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Načíst pravidla (rules.toml)".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayCommand::ReloadRules)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Ukončit Crimp UI".into(),
                activate: Box::new(|this: &mut Self| this.emit(TrayCommand::QuitApp)),
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

            // Dvě protínající se S-křivky (modrá & zelená) + tečky na krajích
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
