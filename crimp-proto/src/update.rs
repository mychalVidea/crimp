use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CRIMP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CRIMP_BUILD_COMMIT: Option<&str> = option_env!("CRIMP_BUILD_COMMIT");
const DEFAULT_REPO: &str = "mychalVidea/crimp";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledManifest {
    pub version: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub build_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub current_commit: Option<String>,
    pub latest_version: String,
    pub latest_commit: Option<String>,
    pub update_available: bool,
    pub source_label: String,
    pub release_url: String,
}

#[must_use]
pub fn installed_manifest_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|base| base.join("crimp").join("installed.json"))
}

#[must_use]
pub fn read_installed_manifest() -> InstalledManifest {
    if let Some(path) = installed_manifest_path()
        && let Ok(raw) = fs::read_to_string(path)
        && let Ok(manifest) = serde_json::from_str::<InstalledManifest>(&raw)
        && !manifest.version.trim().is_empty()
    {
        return InstalledManifest {
            version: CRIMP_VERSION.to_string(),
            commit: manifest
                .commit
                .or_else(|| CRIMP_BUILD_COMMIT.map(str::to_string)),
            build_time: manifest.build_time,
        };
    }

    InstalledManifest {
        version: CRIMP_VERSION.to_string(),
        commit: CRIMP_BUILD_COMMIT.map(str::to_string),
        build_time: None,
    }
}

/// Zkontroluje nejnovější dostupný prepackage na GitHubu (`releases/download/latest/version.json`,
/// pak `releases/latest` API a fallback na `Cargo.toml` v `main`).
pub fn check_for_updates() -> Result<UpdateInfo, String> {
    let repo = std::env::var("CRIMP_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());

    let local = read_installed_manifest();

    // 1. Primárně kontrolujeme manifest z automatického CI prepackage release (`latest`)
    let manifest_url = format!("https://github.com/{repo}/releases/download/latest/version.json");
    if let Ok(output) = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "4",
            "-H",
            "Cache-Control: no-cache",
            "-H",
            "User-Agent: crimp-updater",
            &manifest_url,
        ])
        .output()
        && output.status.success()
        && let Ok(remote) = serde_json::from_slice::<InstalledManifest>(&output.stdout)
    {
        let update_available = is_update_needed(
            &local.version,
            local.commit.as_deref(),
            &remote.version,
            remote.commit.as_deref(),
        );

        return Ok(UpdateInfo {
            current_version: local.version,
            current_commit: local.commit,
            latest_version: remote.version,
            latest_commit: remote.commit,
            update_available,
            source_label: "GitHub Prepackage".to_string(),
            release_url: format!("https://github.com/{repo}/releases/tag/latest"),
        });
    }

    // 2. Tagovaný release přes GitHub API (`v*`)
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

        let update_available = is_newer_version(&clean_latest, &local.version);
        return Ok(UpdateInfo {
            current_version: local.version,
            current_commit: local.commit,
            latest_version: clean_latest,
            latest_commit: None,
            update_available,
            source_label: "GitHub Releases".to_string(),
            release_url: html_url,
        });
    }

    // 3. Fallback na `Cargo.toml` ve větvi `main`
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
    let update_available = is_newer_version(&latest_version, &local.version);

    Ok(UpdateInfo {
        current_version: local.version,
        current_commit: local.commit,
        latest_version,
        latest_commit: None,
        update_available,
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

/// Určí, zda se vyplatí aktualizovat (vyšší semver verze, případně jiný commit při stejné verzi).
/// Pokud jsou verze i commit stejné (nebo lokální verze je novější), vrací `false`.
#[must_use]
pub fn is_update_needed(
    local_version: &str,
    local_commit: Option<&str>,
    remote_version: &str,
    remote_commit: Option<&str>,
) -> bool {
    if is_newer_version(remote_version, local_version) {
        return true;
    }
    if is_newer_version(local_version, remote_version) {
        return false;
    }
    match (local_commit, remote_commit) {
        (Some(l), Some(r)) => !l.trim().is_empty() && !r.trim().is_empty() && l.trim() != r.trim(),
        (None, Some(r)) => !r.trim().is_empty(),
        _ => false,
    }
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

/// Na Linuxu kernel k `/proc/self/exe` připojí ` (deleted)` poté, co `install.sh` / balíčkovač
/// nahradí běžící binárku na disku novou verzí. Tato funkce příponu bezpečně ořeže.
#[must_use]
pub fn strip_linux_deleted_suffix(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(s) => PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(s)),
        None => path.to_path_buf(),
    }
}

/// Najde aktuální spustitelnou binárku (`crimp` nebo `crimp-ui`) i tehdy, když původní proces
/// ještě běží ze smazaného inodu po samo-aktualizaci (`/proc/self/exe -> ... (deleted)`).
#[must_use]
pub fn resolve_live_binary(bin_name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let clean = strip_linux_deleted_suffix(&exe);
        if clean.file_name().and_then(|n| n.to_str()) == Some(bin_name) && clean.is_file() {
            return clean;
        }
        if let Some(candidate) = clean.parent().map(|d| d.join(bin_name))
            && candidate.is_file()
        {
            return candidate;
        }
    }

    let mut candidates = Vec::new();
    if let Some(dir) = dirs::executable_dir() {
        candidates.push(dir.join(bin_name));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin").join(bin_name));
    }
    candidates.push(PathBuf::from("/usr/local/bin").join(bin_name));
    candidates.push(PathBuf::from("/usr/bin").join(bin_name));

    for cand in candidates {
        if cand.is_file() {
            return cand;
        }
    }

    PathBuf::from(bin_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_deleted_suffix_from_updated_linux_exe() {
        let deleted = Path::new("/home/user/.local/bin/crimp-ui (deleted)");
        assert_eq!(
            strip_linux_deleted_suffix(deleted),
            PathBuf::from("/home/user/.local/bin/crimp-ui")
        );
        let normal = Path::new("/usr/bin/crimp-ui");
        assert_eq!(strip_linux_deleted_suffix(normal), normal);
    }

    #[test]
    fn compares_semver_and_commit_accurately() {
        assert!(is_newer_version("0.2.1", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_update_needed(
            "0.2.0",
            Some("abc1234"),
            "0.2.0",
            Some("abc1234")
        ));
        assert!(is_update_needed(
            "0.2.0",
            Some("abc1234"),
            "0.2.0",
            Some("def5678")
        ));
        assert!(!is_update_needed(
            "0.3.0",
            Some("abc1234"),
            "0.2.0",
            Some("def5678")
        ));
    }

    #[test]
    fn extracts_workspace_package_version() {
        let sample = "[workspace.package]\nversion = \"0.2.4\"\nedition = \"2024\"\n";
        assert_eq!(extract_toml_version(sample).as_deref(), Some("0.2.4"));
    }
}
