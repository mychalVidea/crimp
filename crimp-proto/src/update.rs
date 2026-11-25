use serde::{Deserialize, Serialize};
use std::fs;
use std::process::Command;

pub const CRIMP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_REPO: &str = "mychalVidea/crimp";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub source_label: String,
    pub release_url: String,
}

/// Zkontroluje nejnovější dostupnou verzi Crimpu na GitHubu (Releases -> fallback Cargo.toml v `main`).
pub fn check_for_updates() -> Result<UpdateInfo, String> {
    let repo = std::env::var("CRIMP_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());

    let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
    if let Ok(output) = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "4",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: crimp-updater",
            &api_url,
        ])
        .output()
        && output.status.success()
        && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        && let Some(tag) = json.get("tag_name").and_then(|v| v.as_str())
    {
        let clean_latest = tag.trim().trim_start_matches('v').to_string();
        let html_url = json
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("https://github.com/{repo}/releases/latest"));

        return Ok(UpdateInfo {
            current_version: CRIMP_VERSION.to_string(),
            update_available: is_newer_version(&clean_latest, CRIMP_VERSION),
            latest_version: clean_latest,
            source_label: "GitHub Releases".to_string(),
            release_url: html_url,
        });
    }

    let raw_toml_url = format!("https://raw.githubusercontent.com/{repo}/main/Cargo.toml");
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "4", &raw_toml_url])
        .output()
        .map_err(|e| format!("Nelze spustit curl pro kontrolu aktualizací: {e}"))?;

    if !output.status.success() {
        return Err("Nepodařilo se spojit s GitHubem pro kontrolu aktualizací".to_string());
    }

    let toml_text = String::from_utf8_lossy(&output.stdout);
    let latest_version = extract_toml_version(&toml_text)
        .ok_or_else(|| "Vzdálený Cargo.toml neobsahuje platnou verzi".to_string())?;

    Ok(UpdateInfo {
        current_version: CRIMP_VERSION.to_string(),
        update_available: is_newer_version(&latest_version, CRIMP_VERSION),
        latest_version,
        source_label: "GitHub main".to_string(),
        release_url: format!("https://github.com/{repo}"),
    })
}

/// Bezpečně stáhne a spustí oficiální `install.sh` z GitHubu (aktualizuje binárky i systemd službu).
pub fn run_self_update() -> Result<String, String> {
    let repo = std::env::var("CRIMP_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());

    let script_url = format!("https://raw.githubusercontent.com/{repo}/main/install.sh");
    let tmp_path = std::env::temp_dir().join(format!("crimp-update-{}.sh", std::process::id()));

    let dl = Command::new("curl")
        .args(["-fsSL", "--max-time", "15", "-o"])
        .arg(&tmp_path)
        .arg(&script_url)
        .output()
        .map_err(|e| format!("Chyba spuštění curl: {e}"))?;

    if !dl.status.success() {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "Stažení instalátoru selhalo: {}",
            String::from_utf8_lossy(&dl.stderr).trim()
        ));
    }

    let run = Command::new("bash").arg(&tmp_path).output();
    let _ = fs::remove_file(&tmp_path);

    let out = run.map_err(|e| format!("Nelze spustit update skript: {e}"))?;
    if !out.status.success() {
        let err_msg = String::from_utf8_lossy(&out.stderr);
        let std_msg = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "Aktualizace selhala:\n{}\n{}",
            err_msg.trim(),
            std_msg.trim()
        ));
    }

    Ok("Crimp byl úspěšně aktualizován a služba restartována.".to_string())
}

#[must_use]
pub fn extract_toml_version(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version")
            && let Some(val) = rest.trim_start().strip_prefix('=')
        {
            let cleaned = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if parse_semver(&cleaned).is_some() {
                return Some(cleaned);
            }
        }
    }
    None
}

#[must_use]
pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    match (parse_semver(candidate), parse_semver(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let core = raw.trim().trim_start_matches('v').split('-').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_versions_accurately() {
        assert!(is_newer_version("0.2.1", "0.2.0"));
        assert!(is_newer_version("v0.3.0", "0.2.9"));
        assert!(is_newer_version("1.0.0", "0.99.9"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.1.9", "0.2.0"));
    }

    #[test]
    fn extracts_workspace_package_version() {
        let sample = "[workspace.package]\nversion = \"0.2.4\"\nedition = \"2024\"\n";
        assert_eq!(extract_toml_version(sample).as_deref(), Some("0.2.4"));
    }
}
