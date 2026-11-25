# Crimp

![Rust](https://img.shields.io/badge/Rust-1.100%2B%20(edition%202024)-orange?logo=rust)
![Platform](https://img.shields.io/badge/Platform-Linux%20PipeWire-blue)
![License](https://img.shields.io/badge/License-MIT-green)

**Crimp** je ultralehký PipeWire **Patchbay GUI** (alternativa ke `qpwgraph`), **routing/blocklist daemon** a **live DSP/Settings inspektor** pro Linux v čistém Rustu.

---

## Rychlá instalace (jako aplikace z GitHubu)

Jediný příkaz stáhne předkompilovaný balíček z GitHub Releases (případně sestaví ze zdrojů, pokud release ještě není vydán), nainstaluje **`crimp`** + **`crimp-ui`**, přidá ikonu aplikace **Crimp** do systémového menu (`.desktop`) a zapne uživatelskou službu `crimp.service`:

```bash
curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash
```

Jen kontrola kompatibility bez instalace (funguje na CachyOS, Archu, Fedoře, Ultramarine, Debianu i Ubuntu):

```bash
curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash -s -- --check
```

Odinstalování:

```bash
curl -fsSL https://raw.githubusercontent.com/mychalVidea/crimp/main/install.sh | bash -s -- --uninstall
```

Pro **Arch / CachyOS** je připraven [`contrib/PKGBUILD`](contrib/PKGBUILD), pro **Fedoru / Ultramarine** RPM specifikace [`contrib/crimp.spec`](contrib/crimp.spec). Release binárky v GitHub Actions se staví proti konzervativní glibc `2.35` (`ubuntu-22.04`) pro maximální přenositelnost napříč distribucemi.

---

## Proč Crimp vs samotný qpwgraph / WirePlumber

- **`crimp-ui` (nativní `egui` + `glow`, bez Electronu):** Lehké desktopové okno s interaktivním Patchbay grafem (přetahování uzlů, tahání Bézierových kabelů mezi piny, rozpojování pravým kliknutím) a přehledem živých **PipeWire nastavení**.
- **Aktivní ochrana grafu (daemon `crimp`):** Na rozdíl od čistě pasivního patchbaye Crimp trvale hlídá `Registry` události a chrání tvou topologii před nechtěnými auto-linky WirePlumberu (`[[deny]]` + `[[route]]` s `exclusive = true`).
- **Kompletní PipeWire Settings inspektor & ovládání:** Živě čte PipeWire `Core` i `Metadata` (`settings` a `default`) — vidíš přesnou latenci v ms, aktivní quantum, sample rate, `clock.min-quantum`, `clock.max-quantum`, `clock.allowed-rates`, `clock.quantum-limit`, `default.audio.sink`, `default.audio.source` a všechny surové metadata klíče v jedné tabulce.
- **Virtuální audio sběrnice (`[[virtual_sink]]`):** Deklarativní Null Sinky spravované přímo daemonem.
- **Dynamické DSP profily (`[[dsp_profile]]`):** Např. při startu DAW automaticky sníží `clock.force-quantum` na `128`, po ukončení jej vrátí na výchozí.
- **Prekompilované balíčky na GitHub Actions:** Každý push do `main` automaticky sestaví přenosný `.tar.gz` balíček pod rolling tagem `latest` (i tagy `v*`), takže se při `crimp update` nebo tlačítku **Update** v UI nic nemusí kompilovat lokálně (jen pokud je vzdálený build novější než nainstalovaný).
- **Start se systémem & tichá tray ikona (`crimp-ui --tray`):** V *PipeWire Settings* lze jedním klikem zapnout autostart daemona (`crimp.service`) i lehké tray ikony (`~/.config/autostart/crimp-tray.desktop`) — po přihlášení se rovnou načtou všechna pravidla na pozadí a hlavní okno se neotvírá, dokud jej nevyvoláš z lišty nebo menu.
- **Hot-reload & Unix IPC:** Změny v `~/.config/crimp/rules.toml` se načtou automaticky do 2 s nebo okamžitě přes `crimp reload` / tlačítko v UI.

---

## Architektura Workspace

```text
crimp/
├── Cargo.toml                 # Workspace + daemon & CLI (`crimp`)
├── install.sh                 # Jednoduchý GitHub instalátor balíčku a .desktop appky
├── crimp.svg                  # Vektorová ikona aplikace
├── contrib/
│   ├── crimp.desktop          # Launcher do menu aplikací (jako qpwgraph -> crimp-ui)
│   ├── crimp.service          # systemd --user jednotka pro daemon
│   ├── crimp.spec             # Fedora / Ultramarine RPM spec
│   └── PKGBUILD               # Arch / CachyOS balíček
├── .github/workflows/
│   └── release.yml            # CI build & GitHub Releases (.tar.gz + .deb)
├── crimp-proto/               # Lehké jádro bez C závislostí (config, detect, graph, rules, ipc)
├── crimp-ui/                  # Ultralehký nativní GUI Patchbay & Settings (eframe/glow)
└── src/                       # PipeWire daemon + CLI (`main.rs`, `pw_thread.rs`)
```

---

## Ruční sestavení ze zdrojů

Vyžaduje **Rust 1.100+** (`edition = "2024"`), hlavičky PipeWire a `clang`:

- **Arch / CachyOS:** `sudo pacman -S pipewire clang pkgconf`
- **Ubuntu / Debian:** `sudo apt install libpipewire-0.3-dev libspa-0.2-dev clang pkg-config`
- **Fedora:** `sudo dnf install pipewire-devel clang pkgconf-pkg-config`

```bash
cargo build --release --workspace
```

Výstupní binárky: `target/release/crimp` (daemon + CLI) a `target/release/crimp-ui` (GUI).

---

## Konfigurace (`~/.config/crimp/rules.toml`)

```toml
[ipc]
enabled = true

[dsp]
default_quantum = 1024
default_rate = 48000

[pw_settings]
min_quantum = 64
max_quantum = 2048
allowed_rates = [44100, 48000, 96000]
power_of_two_quantum = true

[[virtual_sink]]
name = "crimp_stream_mix"
description = "Crimp Stream Mix"
channels = 2
positions = ["FL", "FR"]

[[deny]]
name = "Zákaz HDMI monitoringu do Discordu"
enabled = true
source = { node_name = "alsa_output.*hdmi*.monitor" }
target = { application_name = "WEBRTC VoiceEngine" }

[[route]]
name = "Firefox do Crimp virtuální sběrnice"
enabled = true
exclusive = true
source = { application_name = "Firefox" }
target = { node_name = "crimp_stream_mix" }

[[dsp_profile]]
name = "DAW low-latency"
enabled = true
priority = 100
source = { application_name = "REAPER" }
quantum = 128
rate = 48000
```

---

## CLI příkazy

| Příkaz | Popis |
| :--- | :--- |
| `crimp` / `crimp daemon` | Spustí PipeWire daemon (včetně auto-reconnectu při restartu PipeWire) |
| `crimp ui` | Otevře grafickou aplikaci `crimp-ui` |
| `crimp doctor` | Ověří distribuci, `libpipewire-0.3`, socket, display a detekuje audio hardware |
| `crimp auto [--apply]` | Automaticky navrhne (a s `--apply` uloží) bezpečná pravidla pro daný PC |
| `crimp update [--check] [--force]` | Zkontroluje a nainstaluje nejnovější verzi z GitHubu (včetně restartu služby) |
| `crimp settings [--json]` | Vypíše všechna živá PipeWire `Core` & `Metadata` nastavení a latenci |
| `crimp set-setting <KLÍČ> <VAL\|auto>` | Živě změní PipeWire `settings` klíč (např. `clock.min-quantum 64`) |
| `crimp set-default-sink <NAME\|auto>` | Přepne `default.audio.sink` |
| `crimp set-default-source <NAME\|auto>` | Přepne `default.audio.source` |
| `crimp set-quantum <N\|auto>` | Živě přepne `clock.force-quantum` |
| `crimp set-rate <N\|auto>` | Živě přepne `clock.force-rate` |
| `crimp status [--json]` | Stav běžícího daemona a počty blokací/rout |
| `crimp graph [--json]` | Živý snímek uzlů, portů a linků |
| `crimp block-list [--json]` | Posledních až 128 zablokovaných auto-linků |
| `crimp check` | Offline validace `rules.toml` |
| `crimp reload` | Okamžitý reload pravidel |
| `crimp shutdown` | Čisté ukončení daemona |

---

## Licence

MIT — viz [`LICENSE`](LICENSE).
