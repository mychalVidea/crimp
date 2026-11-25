use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutostartState {
    pub daemon_enabled: bool,
    pub tray_enabled: bool,
}

impl AutostartState {
    #[must_use]
    pub fn is_fully_enabled(&self) -> bool {
        self.daemon_enabled && self.tray_enabled
    }
}

#[must_use]
pub fn tray_autostart_desktop_path() -> Option<PathBuf> {
    dirs::config_dir().map(|cfg| cfg.join("autostart").join("crimp-tray.desktop"))
}

#[must_use]
pub fn probe_autostart() -> AutostartState {
    let daemon_enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", "crimp.service"])
        .output()
        .is_ok_and(|out| out.status.success());

    let tray_enabled = tray_autostart_desktop_path().is_some_and(|p| p.exists());

    AutostartState {
        daemon_enabled,
        tray_enabled,
    }
}

/// Zapne nebo vypne automatické spuštění Crimpu po přihlášení:
/// - `crimp.service` načte pravidla, virtuální sinky a PipeWire limity na pozadí,
/// - `crimp-tray.desktop` spustí jen tichou tray ikonu (`--tray`) bez otevírání GUI okna.
pub fn set_autostart(enable: bool) -> Result<String, String> {
    let desktop_path = tray_autostart_desktop_path()
        .ok_or_else(|| "Nelze určit cestu k ~/.config/autostart".to_string())?;

    if enable {
        let ui_exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "crimp-ui".to_string());

        if let Some(parent) = desktop_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Chyba vytvoření autostartu: {e}"))?;
        }

        let content = format!(
            "[Desktop Entry]\n\
            Type=Application\n\
            Version=1.0\n\
            Name=Crimp Tray\n\
            Comment=Tichá ikona Crimpu v oznamovací oblasti (bez otevření okna při startu)\n\
            Exec={ui_exe} --tray\n\
            Icon=crimp\n\
            Terminal=false\n\
            X-GNOME-Autostart-enabled=true\n"
        );
        fs::write(&desktop_path, content)
            .map_err(|e| format!("Nelze zapsat autostart soubor: {e}"))?;

        let status = Command::new("systemctl")
            .args(["--user", "enable", "--now", "crimp.service"])
            .output()
            .map_err(|e| format!("Nelze spustit systemctl: {e}"))?;

        if !status.status.success() {
            let err = String::from_utf8_lossy(&status.stderr);
            return Err(format!(
                "Tray autostart přidán, ale systemd služba selhala: {}",
                err.trim()
            ));
        }

        Ok("Start s PC zapnut: pravidla i PipeWire limity se načtou na pozadí a v liště bude jen tray ikona (okno se samo neotevře).".into())
    } else {
        if desktop_path.exists() {
            fs::remove_file(&desktop_path)
                .map_err(|e| format!("Nelze odstranit autostart soubor: {e}"))?;
        }
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "crimp.service"])
            .output();

        Ok("Automatické spuštění při startu PC vypnuto.".into())
    }
}
