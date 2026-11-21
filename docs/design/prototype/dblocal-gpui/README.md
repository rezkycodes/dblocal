# DBLocal — prototipe UI (GPUI Kit)

Implementasi desain DBLocal dengan **[GPUI Kit](https://gpui-kit.com/docs/) 0.10** (GPUI dari Zed + `gpui-component`), tema gelap berbasis **#15161d**.

> Status: **prototipe UI**. Semua data instance adalah data contoh di memori.
> Start/Stop, unduhan runtime, dan pembuatan database disimulasikan — belum ada
> proses engine sungguhan. Lapisan `dblocal-core` (katalog, unduhan + SHA-256,
> EngineAdapter, state.db) dari PRD menyusul dan akan menggantikan `src/model.rs`.

## Yang sudah bisa dicoba

| Fitur PRD | Di aplikasi |
|---|---|
| D-23 Workspace gaya Zed | Title bar, dock Instances (pohon per engine, filter, collapse), tab per instance, status bar |
| F-INS-03 Start/Stop | Tombol Start/Stop/Restart dengan transisi `starting`/`stopping` |
| F-CRD-02 Copy URI / .env | Menyalin ke clipboard sungguhan + notifikasi; tombol Tampilkan/Sembunyikan password |
| F-INS-01 + F-CAT-02 + F-PRT-01 | Dialog **Instance baru**: pilih engine & versi (badge *Terpasang*), nama & port terisi otomatis, validasi nama/port, opsi tanpa password |
| F-RTM-01 | Versi yang belum terpasang masuk state *Provisioning* dengan progres unduhan |
| F-LOG-01/02 | Tab Logs + banner diagnosa "Port sedang dipakai" → **Pindah ke port berikutnya & start** (coba di `pg-shop`) |
| F-DBU | Tab Databases: daftar, buat database (validasi `[A-Za-z0-9_]`), user & hak, reset password |
| F-CFG | Tab Config: nilai saat ini vs default, banner restart |
| F-PLH-02 | Status tidak hanya warna: ● running · ○ stopped · ▲ failed · ◐ sibuk |

Pintasan: `Ctrl/Cmd+N` instance baru · `Ctrl/Cmd+Shift+C` copy URI · `Ctrl/Cmd+Enter` start/stop.

## Menjalankan

Butuh **Rust 1.92+** (`rustup update`).

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
Pasang Visual Studio 2022 Build Tools (workload *Desktop development with C++*), CMake, dan toolchain Rust **MSVC**, lalu:
```powershell
cargo run --release
```

Build pertama mengompilasi GPUI dan butuh beberapa menit; build berikutnya cepat.

Binary siap pakai untuk tiga OS bisa dibuat otomatis: push repo ini ke GitHub, workflow
`.github/workflows/build.yml` akan menghasilkan artefak `dblocal-linux-x86_64`,
`dblocal-macos-arm64`, dan `dblocal-windows-x86_64.exe` di tab *Actions*.

## Struktur

```
dblocal-gpui/
├─ Cargo.toml                # gpui-kit = "=0.10.0" (dipin, pre-1.0)
├─ themes/dblocal.json       # tema "DBLocal Dark" (#15161d) untuk komponen gpui-kit
├─ src/
│  ├─ main.rs                # bootstrap: application(), init, tema, keybinding, jendela
│  ├─ assets.rs              # ikon bawaan + ikon Lucide tambahan (icon_assets!)
│  ├─ theme.rs               # load tema + token warna untuk elemen kustom
│  ├─ model.rs               # EngineKind, Status, Instance, data contoh
│  └─ ui/
│     ├─ workspace.rs        # state, aksi, title bar, tab instance, header, status bar
│     ├─ sidebar.rs          # dock Instances + Runtimes
│     ├─ pages.rs            # Overview, Logs, Databases, Config
│     ├─ new_instance.rs     # dialog Instance baru
│     └─ components.rs       # status_dot, engine_badge, card, banner, …
└─ .github/workflows/build.yml
```

## Mengganti warna tema

Semua warna komponen ada di `themes/dblocal.json` (kunci mengikuti skema tema
gpui-kit). Warna elemen buatan sendiri ada di `src/theme.rs` → `tokens`.
Pertahankan rasio kontras teks ≥ 4,5:1 (F-PLH-02).

## Belum termasuk

- Tray/menubar dan dialog Quit (F-TRY) — GPUI belum punya API tray; ini spike F-SPK-04.
- Command palette `Ctrl+K`, tema terang, hapus instance, Settings › Storage.
- Integrasi engine sungguhan (`dblocal-core`).

## Lisensi

GPL-3.0-only (D-05). GPUI Kit, `gpui-component`, dan GPUI berlisensi Apache-2.0; ikon Lucide berlisensi ISC.
