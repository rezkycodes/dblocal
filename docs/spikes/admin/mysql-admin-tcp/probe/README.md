# Probe: MySQL/MariaDB admin transport type-checked for Windows

`cargo check -p dblocal-core --target x86_64-pc-windows-gnu` cannot run on this
host: it stops in `aws-lc-sys`, `libz-sys`, `libsqlite3-sys` and `liblzma-sys`,
whose build scripts need `x86_64-w64-mingw32-gcc` (not installed, and no sudo to
install it). This probe compiles the touched module —
`crates/core/src/admin/mysql.rs` — against the **real** `mysql_async` 0.37.1 for
`x86_64-pc-windows-gnu`, with only its neighbours stubbed:

| Real (`#[path]`, never copied) | Stubbed |
|---|---|
| `admin/mysql.rs` (the module under test) | `secret::SecretString` (same shape, no zeroize) |
| `admin/sql.rs`, `admin/error.rs`, `admin/reset.rs`, `admin/executor.rs`, `admin/messages.rs` | `secrets::generate_password` |
| | `crate::error::Result` |
| | `crate::service::UserFacingError` |
| | `admin/postgres.rs` (`DatabaseInfo`, `UserInfo`; copied getters only) |

## Run

```sh
cd docs/spikes/admin/mysql-admin-tcp/probe
CARGO_TARGET_DIR=$HOME/.cache/dblocal-probe-target \
  cargo check --target x86_64-pc-windows-gnu
```

`src/lib.rs` has a `compile_error!` on `cfg(unix)`, so a host build cannot
silently pass by type-checking the unix arm.

## What it proves, and what it does not

- Proves: the module compiles for a **non-unix** target, so no unix-only API is
  reached and `cfg!(unix)` in `MysqlTarget::new` selects the loopback arm there.
- Does **not** prove: that a Windows `mysqld`/`mariadbd` accepts the connection,
  that the `caching_sha2_password` RSA exchange succeeds against a real Windows
  server, or anything about the engine's own `my.ini` on Windows. No Windows
  machine is available.

`default-features = false, features = ["minimal-rust"]` keeps `flate2` on its
pure-Rust backend so the probe needs no C toolchain of its own. The product
build's feature set (workspace `mysql_async = "0.37"`, default features) is not
affected: this crate is not a workspace member.
