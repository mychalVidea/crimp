# Crimp

> **Ultralehký PipeWire routing & blocklist daemon pro Linux.**  
> Žádné nechtěné automatické linky od WirePlumberu. Okamžité vynucení pravidel v reálném čase nad PipeWire audio grafem.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-orange.svg)](https://www.rust-lang.org/)
[![PipeWire](https://img.shields.io/badge/PipeWire-0.3%2B-green.svg)](https://pipewire.org/)

---

## Proč Crimp vznikl (řešený problém)

WirePlumber je výchozí správce relací pro PipeWire a dělá skvělou práci pro běžné použití – automaticky propojuje nové aplikace na výchozí výstup a spojuje mikrofony/loopbacky. Při pokročilejším směrování zvuku (streaming, OBS, oddělené kanály pro Discord/hry/hudbu, DAW, virtuální linky) se ale často stává, že:

- WirePlumber **agresivně znovu vytváří nechtěné linky** i poté, co je ručně odpojíš v `qpwgraph` nebo `helvum`.
- Restart aplikace nebo přepojení USB rozhraní **resetuje pracně nastavené routování** do výchozího stavu.
- Session manager spojuje uzly, které se **nesmí nikdy propojit** (např. loopback/monitor stopy do hlasového vstupu nebo nechtěný mikrofon do stream mixu).

**Crimp** neběží místo PipeWire – běží vedle WirePlumberu jako tichý, nativní daemon napsaný v Rustu. Neustále sleduje globální události PipeWire registru, udržuje si mapu názvů uzlů/portů a **v okamžiku vzniku nežádoucího linku jej okamžitě rozpojí** (`destroy_global`), zatímco připravený engine umožňuje vynucovat vlastní směrování podle deklarativních pravidel.

---

## Architektura

Protože objekty v `pipewire-rs` (`MainLoopRc`, `ContextRc`, `Core`, `Registry`) obalují nativní C struktury `libpipewire` a nejsou `Send`/`Sync`, je architektura navržena s čistým oddělením vláken:

```text
┌─────────────────────────────┐         pipewire::channel          ┌───────────────────────────────────┐
│        Main OS Thread       │ ─────────────────────────────────► │      PipeWire Thread (crimp-pw)   │
│                             │     PwCommand::ReloadConfig /      │                                   │
│ • tracing logger            │     PwCommand::Shutdown            │ • pipewire::main_loop::MainLoopRc │
│ • rules.toml loader         │                                    │ • ContextRc / Core / Registry     │
│ • SIGINT / SIGTERM handler  │                                    │ • global / global_remove listener │
│ • (budoucí IPC & CLI/GUI)   │                                    │ • AudioGraph (ID -> názvy/porty)  │
└─────────────────────────────┘                                    │ • RuleEngine -> destroy_global()  │
                                                                   └───────────────────────────────────┘
```

### Rozdělení modulů v `src/`

| Modul | Odpovědnost |
| :--- | :--- |
| `src/main.rs` | Inicializace `tracing` loggeru, načtení `~/.config/crimp/rules.toml`, spuštění dedikovaného PipeWire vlákna a graceful shutdown na `SIGINT`/`SIGTERM`. |
| `src/config.rs` | Načtení a serializace pravidel (`deny`, `route`) přes `serde` + `toml`, automatické vytvoření defaultního `~/.config/crimp/rules.toml` při prvním spuštění. |
| `src/graph.rs` | In-memory stavová mapa `AudioGraph` převádějící dočasná číselná ID PipeWire na čitelná metadata (`node.name`, `application.name`, `media.class`, `port.name`). |
| `src/rules.rs` | Vyhodnocovací engine `RuleEngine` podporující wildcard (`*`) shody nad zdrojem/cílem a generování akcí (`DestroyLink`, příprava pro `EnsureRoute`). |
| `src/pw_thread.rs` | Nativní PipeWire event loop na samostatném OS vlákně, synchronní poslech `global` / `global_remove`, komunikace přes `pipewire::channel` a okamžité ničení blokovaných linků. |

---

## Požadavky pro sestavení

Crimp používá oficiální vazby [`pipewire`](https://crates.io/crates/pipewire) (`pipewire-rs`), které při kompilaci generují FFI přes `bindgen`:

- **Rust toolchain** (`rustc` + `cargo`, edition 2021)
- **PipeWire vývojové hlavičky** (`libpipewire-0.3` / `pkg-config`)
- **Clang / `libclang`** (pro `bindgen`)

Instalace závislostí podle distribuce:

```bash
# Ubuntu / Debian / Pop!_OS
sudo apt install libpipewire-0.3-dev clang pkg-config

# Arch Linux / CachyOS
sudo pacman -S pipewire clang pkgconf

# Fedora
sudo dnf install pipewire-devel clang pkgconf-pkg-config
```

---

## Sestavení a spuštění

```bash
# Klonování repozitáře (po nahrání na GitHub)
git clone <URL_TVEHO_REPOZITARE>
cd crimp

# Ověření sestavení / release build
cargo check
cargo build --release

# Spuštění daemona (vypisuje info logy přes tracing)
RUST_LOG=crimp=info ./target/release/crimp
```

Pro ladění všech příchozích PipeWire objektů a detailní stopování pravidel použij:

```bash
RUST_LOG=crimp=debug cargo run
```

---

## Konfigurace (`~/.config/crimp/rules.toml`)

Při prvním spuštění Crimp automaticky vytvoří výchozí soubor `~/.config/crimp/rules.toml`. Pravidla podporují case-insensitive wildcard vzory (`*`):

```toml
# Pravidlo blocklistu: okamžitě rozpojí jakýkoli link mezi zdrojem a cílem
[[deny]]
name = "block-obs-monitor-to-mic"
enabled = true

[deny.source]
app_name = "OBS*"
# volitelně: node_name, media_class, port_name

[deny.target]
media_class = "Audio/Sink"
node_name = "*virtual-mic*"

# Pravidlo směrování (připraveno pro automatické vynucení sinku)
[[route]]
name = "discord-to-comms-sink"
enabled = false

[route.source]
app_name = "Discord*"

[route.target]
node_name = "crimp-comms-sink"
```

### Dostupné klíče v `source` / `target` matcherech
- `node_name` – PipeWire vlastnost `node.name` (např. `alsa_output.pci-...`)
- `app_name` – PipeWire vlastnost `application.name` (např. `Firefox`, `Discord`, `OBS`)
- `media_class` – PipeWire vlastnost `media.class` (např. `Stream/Output/Audio`, `Audio/Sink`)
- `port_name` – PipeWire vlastnost `port.name` (např. `output_FL`, `playback_FR`)

---

## Roadmapa

- [x] **MVP Core:** Dedikované PipeWire vlákno (`MainLoopRc`), `AudioGraph` mapa, `deny` blocklist s okamžitým `destroy_global`.
- [ ] **Plná `route` automatizace:** Aktivní vytváření a přepojování linků (`core.create_object::<Link>()`) při objevení zdrojového nebo cílového uzlu + hot-reload přes `SIGHUP` / inotify.
- [ ] **Virtuální uzly (Null Sinks):** Deklarativní vytváření persistentních `Audio/Sink` a `Audio/Duplex` kanálů (Chat, Game, Music, Stream) bez externích skriptů.
- [ ] **Live DSP parametry:** Dynamické přepínání `clock.force-quantum` a `clock.force-rate` podle spuštěných aplikací (např. nízká latence pro DAW, úspora pro prohlížeč).
- [ ] **IPC Socket & GUI/TUI:** Lokální Unix domain socket pro inspekci grafu, živé logy odpojených linků a desktopového/terminálového klienta.
- [ ] **Systemd user unit:** Připravený `crimp.service` pro spouštění při přihlášení (`systemctl --user enable --now crimp`).

---

## Licence

Projekt je šířen pod licencí [MIT](LICENSE).
