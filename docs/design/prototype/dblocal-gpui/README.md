# DBLocal — UI prototype (GPUI Kit)

A DBLocal design implementation with **[GPUI Kit](https://gpui-kit.com/docs/) 0.10** (Zed's GPUI + `gpui-component`), dark theme based on **#15161d**.

> Status: **UI prototype**. All instance data is in-memory sample data.
> Start/Stop, runtime downloads and database creation are simulated — there is no
> real engine process yet. The `dblocal-core` layer from the PRD (catalog, download
> + SHA-256, EngineAdapter, state.db) follows and will replace `src/model.rs`.

## What you can try

| PRD feature | In the app |
|---|---|
| D-23 Zed-style workspace | Title bar, Instances dock (tree per engine, filter, collapse), tab per instance, status bar |
| F-INS-03 Start/Stop | Start/Stop/Restart buttons with `starting`/`stopping` transitions |
| F-CRD-02 Copy URI / .env | Copies to the real clipboard + a notification; Show/Hide password toggle |
| F-INS-01 + F-CAT-02 + F-PRT-01 | **New instance** dialog: pick engine & version (Installed badge), pre-filled name & port, name/port validation, no-password option |
| F-RTM-01 | A version that is not installed yet enters the *Provisioning* state with download progress |
| F-LOG-01/02 | Logs tab + a "Port is already in use" diagnosis banner → **Move to the next port and start** (try `pg-shop`) |
| F-DBU | Databases tab: list, create a database (validate `[A-Za-z0-9_]`), users & privileges, reset password |
| F-CFG | Config tab: current vs default values, restart banner |
| F-PLH-02 | Status is never colour only: ● running · ○ stopped · ▲ failed · ◐ busy |

Shortcuts: `Ctrl/Cmd+N` new instance · `Ctrl/Cmd+Shift+C` copy URI · `Ctrl/Cmd+Enter` start/stop.

## Running

Needs **Rust 1.92+** (`rustup update`).

### Linux (Fedora)
```bash
sudo dnf install -y gcc gcc-c++ clang fontconfig-devel wayland-devel \
  libxkbcommon-x11-devel libxcb-devel openssl-devel libzstd-devel vulkan-loader
cargo run --release
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install -y gcc g++ clang libfontconfig-dev libwayland-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libssl-dev libzstd-dev libvulkan1
cargo run --release
```

### macOS (13+)
```bash
xcode-select --install
cargo run --release
```

### Windows 10/11
Install Visual Studio 2022 Build Tools (the *Desktop development with C++* workload), CMake, and the **MSVC** Rust toolchain, then:
```powershell
cargo run --release
```

The first build compiles GPUI and takes a few minutes; later builds are fast.

Ready-to-use binaries for three OSes can be built automatically: push this repo to GitHub and
the `.github/workflows/build.yml` workflow produces `dblocal-linux-x86_64`,
`dblocal-macos-arm64` and `dblocal-windows-x86_64.exe` artifacts under the *Actions* tab.

## Structure

```
dblocal-gpui/
├─ Cargo.toml                # gpui-kit = "=0.10.0" (pinned, pre-1.0)
├─ themes/dblocal.json       # "DBLocal Dark" theme (#15161d) for the gpui-kit components
├─ src/
│  ├─ main.rs                # bootstrap: application(), init, theme, keybindings, window
│  ├─ assets.rs              # bundled icons + extra Lucide icons (icon_assets!)
│  ├─ theme.rs               # load the theme + colour tokens for custom elements
│  ├─ model.rs               # EngineKind, Status, Instance, sample data
│  └─ ui/
│     ├─ workspace.rs        # state, actions, title bar, instance tabs, header, status bar
│     ├─ sidebar.rs          # Instances + Runtimes dock
│     ├─ pages.rs            # Overview, Logs, Databases, Config
│     ├─ new_instance.rs     # New instance dialog
│     └─ components.rs       # status_dot, engine_badge, card, banner, …
└─ .github/workflows/build.yml
```

## Changing the theme colours

Every component colour lives in `themes/dblocal.json` (keys follow the gpui-kit theme
schema). Colours for our own elements live in `src/theme.rs` → `tokens`.
Keep the text contrast ratio at 4.5:1 or better (F-PLH-02).

## Not included

- Tray/menubar and the Quit dialog (F-TRY) — GPUI has no tray API yet; this is spike F-SPK-04.
- The `Ctrl+K` command palette, the light theme, deleting an instance, Settings › Storage.
- Real engine integration (`dblocal-core`).

## License

GPL-3.0-only (D-05). GPUI Kit, `gpui-component` and GPUI are Apache-2.0; the Lucide icons are ISC.
