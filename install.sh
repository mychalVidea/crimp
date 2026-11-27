#!/usr/bin/env bash
set -euo pipefail

REPO="${CRIMP_REPO:-mychalVidea/crimp}"
BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"
DATA_DIR="${HOME}/.local/share/crimp"
SERVICE_DIR="${HOME}/.config/systemd/user"

ACTION="install"
for arg in "$@"; do
  case "$arg" in
    --uninstall|uninstall) ACTION="uninstall" ;;
    --check|check) ACTION="check" ;;
    --help|-h)
      cat <<'EOF'
Crimp instalátor (stáhne předkompilovaný balíček z GitHubu nebo sestaví ze zdrojů)

Použití:
  curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash -s -- --check
  curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash -s -- --uninstall
EOF
      exit 0
      ;;
  esac
done

log() { printf '\033[1;34m[crimp]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[crimp]\033[0m %s\n' "$*" >&2; }
err() { printf '\033[1;31m[crimp]\033[0m %s\n' "$*" >&2; }

detect_distro() {
  DISTRO_ID="linux"
  DISTRO_LIKE=""
  DISTRO_PRETTY="Linux"
  if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    DISTRO_ID="${ID:-linux}"
    DISTRO_LIKE="${ID_LIKE:-}"
    DISTRO_PRETTY="${PRETTY_NAME:-$DISTRO_ID}"
  fi
  DISTRO_KEY="$(printf '%s %s' "$DISTRO_ID" "$DISTRO_LIKE" | tr '[:upper:]' '[:lower:]')"
}

source_deps_hint() {
  case "$DISTRO_KEY" in
    *cachyos*|*arch*|*manjaro*|*endeavour*)
      echo "sudo pacman -S --needed pipewire wireplumber clang pkgconf rustup"
      ;;
    *ultramarine*|*fedora*|*nobara*|*bazzite*)
      echo "sudo dnf install pipewire-devel clang pkgconf-pkg-config cargo rust"
      ;;
    *ubuntu*|*debian*|*pop*|*mint*)
      echo "sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang pkg-config"
      ;;
    *suse*)
      echo "sudo zypper install pipewire-devel clang pkg-config"
      ;;
    *)
      echo "Nainstalujte vývojové balíčky libpipewire-0.3, clang, pkg-config a Rust"
      ;;
  esac
}

find_libpipewire() {
  for p in \
    /usr/lib/libpipewire-0.3.so.0 \
    /usr/lib64/libpipewire-0.3.so.0 \
    /lib/x86_64-linux-gnu/libpipewire-0.3.so.0 \
    /usr/lib/x86_64-linux-gnu/libpipewire-0.3.so.0 \
    /lib/aarch64-linux-gnu/libpipewire-0.3.so.0 \
    /usr/lib/aarch64-linux-gnu/libpipewire-0.3.so.0; do
    [ -e "$p" ] && echo "$p" && return 0
  done
  if command -v ldconfig >/dev/null 2>&1 && ldconfig -p 2>/dev/null | grep -q 'libpipewire-0\.3\.so\.0'; then
    echo "ldconfig"
    return 0
  fi
  return 1
}

if [ "$ACTION" = "uninstall" ]; then
  log "Odinstalovávám Crimp..."
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now crimp.service 2>/dev/null || true
  fi
  pkill -f "crimp-ui --tray" 2>/dev/null || true
  rm -f "${BIN_DIR}/crimp" "${BIN_DIR}/crimp-ui" \
        "${APP_DIR}/crimp.desktop" \
        "${HOME}/.config/autostart/crimp-tray.desktop" \
        "${ICON_DIR}/crimp.svg" \
        "${SERVICE_DIR}/crimp.service" \
        "${DATA_DIR}/installed.json"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
  fi
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${APP_DIR}" 2>/dev/null || true
  log "Hotovo. Konfigurace v ~/.config/crimp/rules.toml byla zachována."
  exit 0
fi

OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" != "Linux" ]; then
  err "Crimp podporuje výhradně Linux (PipeWire)."
  exit 1
fi

detect_distro

if [ "$ACTION" = "check" ]; then
  log "Kontrola kompatibility pro Crimp (bez instalace):"
  echo "  Distribuce      : ${DISTRO_PRETTY} (id=${DISTRO_ID})"
  echo "  Architektura    : ${ARCH}"
  echo "  Display         : ${XDG_SESSION_TYPE:-neznámé} (WAYLAND=${WAYLAND_DISPLAY:-ne}, DISPLAY=${DISPLAY:-ne})"
  if LPW="$(find_libpipewire)"; then
    echo "  libpipewire-0.3 : OK (${LPW})"
  else
    echo "  libpipewire-0.3 : CHYBÍ"
  fi
  if [ -n "${XDG_RUNTIME_DIR:-}" ] && [ -S "${XDG_RUNTIME_DIR}/pipewire-0" ]; then
    echo "  PipeWire socket : OK (${XDG_RUNTIME_DIR}/pipewire-0)"
  else
    echo "  PipeWire socket : NEBĚŽÍ / nenalezeno"
  fi
  if command -v wireplumber >/dev/null 2>&1 || pgrep -x wireplumber >/dev/null 2>&1; then
    echo "  WirePlumber     : OK"
  else
    echo "  WirePlumber     : nedetekován (není nutný, pokud používáte jiný session manager)"
  fi
  if command -v systemctl >/dev/null 2>&1 && systemctl --user status >/dev/null 2>&1; then
    echo "  systemd --user  : OK"
  else
    echo "  systemd --user  : neaktivní"
  fi
  echo "  Zdrojové deps   : $(source_deps_hint)"
  exit 0
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

install_bundle() {
  local dir="$1"
  mkdir -p "${BIN_DIR}" "${APP_DIR}" "${ICON_DIR}" "${SERVICE_DIR}" "${DATA_DIR}"

  install -Dm755 "${dir}/crimp" "${BIN_DIR}/crimp"
  install -Dm755 "${dir}/crimp-ui" "${BIN_DIR}/crimp-ui"
  install -Dm644 "${dir}/crimp.svg" "${ICON_DIR}/crimp.svg"
  install -Dm644 "${dir}/contrib/crimp.service" "${SERVICE_DIR}/crimp.service"
  if [ -f "${dir}/version.json" ]; then
    install -Dm644 "${dir}/version.json" "${DATA_DIR}/installed.json"
  fi

  sed \
    -e "s|^Exec=.*|Exec=${BIN_DIR}/crimp-ui|" \
    -e "s|^Icon=.*|Icon=${ICON_DIR}/crimp.svg|" \
    "${dir}/contrib/crimp.desktop" > "${APP_DIR}/crimp.desktop"
  chmod 644 "${APP_DIR}/crimp.desktop"

  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true

  if command -v systemctl >/dev/null 2>&1 && systemctl --user status >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable crimp.service >/dev/null 2>&1 || true
    if ! systemctl --user is-active --quiet crimp.service 2>/dev/null; then
      "${BIN_DIR}/crimp" shutdown >/dev/null 2>&1 || true
      sleep 0.3
    fi
    systemctl --user restart crimp.service
    log "Systemd služba crimp.service byla spuštěna/restartována s novou verzí."
  else
    warn "systemd --user neběží; daemona můžeš spustit ručně příkazem: ${BIN_DIR}/crimp daemon"
  fi
}

log "Detekován systém: ${DISTRO_PRETTY} (${ARCH})"
if ! find_libpipewire >/dev/null; then
  warn "V systému nebyla nalezena runtime knihovna libpipewire-0.3.so.0 – nainstalujte prosím PipeWire."
fi

PREPACKAGE_URL="https://github.com/${REPO}/releases/download/latest/crimp-linux-${ARCH}.tar.gz"
RELEASE_URL="https://github.com/${REPO}/releases/latest/download/crimp-linux-${ARCH}.tar.gz"
log "Stahuji předkompilovaný balíček z GitHub Releases..."

if curl -fsSL "${PREPACKAGE_URL}" -o "${TMP_DIR}/crimp-release.tar.gz" 2>/dev/null \
  || curl -fsSL "${RELEASE_URL}" -o "${TMP_DIR}/crimp-release.tar.gz" 2>/dev/null; then
  tar -xzf "${TMP_DIR}/crimp-release.tar.gz" -C "${TMP_DIR}"
  BUNDLE_DIR="${TMP_DIR}/crimp-linux-${ARCH}"
  if [ -x "${BUNDLE_DIR}/crimp" ] && [ -x "${BUNDLE_DIR}/crimp-ui" ]; then
    if "${BUNDLE_DIR}/crimp" --version >/dev/null 2>&1 \
      && "${BUNDLE_DIR}/crimp-ui" --version >/dev/null 2>&1; then
      install_bundle "${BUNDLE_DIR}"
      log "Crimp byl úspěšně nainstalován z předkompilovaného balíčku."
      log "Spusť 'crimp doctor' nebo otevři aplikaci 'Crimp' v menu."
      exit 0
    else
      warn "Předkompilovaný balíček vyžaduje novější systémové knihovny (glibc 2.39+ / libpipewire); sestavuji ze zdrojů..."
    fi
  fi
fi

warn "Předkompilovaný release balíček pro '${ARCH}' nebyl nalezen; sestavuji ze zdrojů z GitHubu..."

if ! command -v cargo >/dev/null 2>&1; then
  err "Chybí Rust/Cargo. Nainstaluj toolchain přes https://rustup.rs/ nebo počkej na vydání GitHub Release."
  exit 1
fi
if ! pkg-config --exists libpipewire-0.3 2>/dev/null || ! command -v clang >/dev/null 2>&1; then
  err "Chybí vývojové hlavičky libpipewire-0.3 nebo clang."
  echo "  Doporučený příkaz: $(source_deps_hint)" >&2
  exit 1
fi

SRC_TARBALL="https://github.com/${REPO}/archive/refs/heads/main.tar.gz"
log "Stahuji zdrojový archiv z ${SRC_TARBALL}..."
curl -fsSL "${SRC_TARBALL}" -o "${TMP_DIR}/src.tar.gz"
tar -xzf "${TMP_DIR}/src.tar.gz" -C "${TMP_DIR}"

SRC_DIR="$(find "${TMP_DIR}" -maxdepth 1 -type d -name 'crimp-*' | head -n 1)"
if [ -z "${SRC_DIR}" ] || [ ! -f "${SRC_DIR}/Cargo.toml" ]; then
  err "Nepodařilo se rozbalit zdrojový repozitář."
  exit 1
fi

log "Kompiluji crimp & crimp-ui (--release --workspace)..."
cargo build --release --workspace --manifest-path "${SRC_DIR}/Cargo.toml"

STAGE="${TMP_DIR}/stage"
mkdir -p "${STAGE}/contrib"
install -Dm755 "${SRC_DIR}/target/release/crimp" "${STAGE}/crimp"
install -Dm755 "${SRC_DIR}/target/release/crimp-ui" "${STAGE}/crimp-ui"
install -Dm644 "${SRC_DIR}/crimp.svg" "${STAGE}/crimp.svg"
install -Dm644 "${SRC_DIR}/contrib/crimp.desktop" "${STAGE}/contrib/crimp.desktop"
install -Dm644 "${SRC_DIR}/contrib/crimp.service" "${STAGE}/contrib/crimp.service"

install_bundle "${STAGE}"
log "Crimp byl úspěšně sestaven a nainstalován! Spusť 'crimp doctor' nebo otevři aplikaci 'Crimp' v menu."
