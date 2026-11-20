# Valkey Engine Spike (Linux x86_64)

Date: 2026-11-20  
Machine: Fedora Linux 44 (Workstation Edition), kernel `7.3.0-cachyos1.fc44.x86_64`, glibc **2.43**.  
Scope: PRD F-SPK-03 adjacent engine lifecycle proof for Valkey, F-INS-02..04, F-LOG-02, F-PRT-02, D-12, D-13, D-16, D-17, and **ASSUMPTION-02**.  
Safety: ports **41000-41999 only** (41300-415xx used), all work in `mktemp -d`, archives cached in `$HOME/.cache/dblocal-dev/`, every started process stopped and temp dirs removed via `trap`, no sudo.

Reproducible scripts (run from repo root):

```sh
bash spikes/engines/valkey/01_download_layout.sh   # download noble+jammy, SHA-256, layout, ldd, glibc floor
bash spikes/engines/valkey/02_lifecycle.sh         # config, detached start, PING, stop/kill-9, redis/valkey URIs
bash spikes/engines/valkey/03_failures.sh          # verbatim failure texts + exit codes
```

---

## 1. Source, download, SHA-256, layout, size

Source: `https://download.valkey.io/releases/` (Valkey 9.2.0, published 2026-11-19 in GitHub releases but binaries are hosted at `download.valkey.io`, **not** as GitHub release assets). Checksums are separate `*.tar.gz.sha256` files at the same base URL.

| Variant | Asset | Archive bytes | SHA-256 (verified) | Entries | `strip_prefix` |
|---|---|---:|---|---:|---|
| Ubuntu noble (x86_64) | `valkey-9.2.0-noble-x86_64.tar.gz` | 45,651,721 (~43.5 MiB) | `871066be4353d97ea8adaa9d6646b41ca94c3def37c57777946fae76e465741c` | 11 | `valkey-9.2.0-noble-x86_64/` |
| Ubuntu jammy (x86_64) | `valkey-9.2.0-jammy-x86_64.tar.gz` | 46,759,925 (~44.6 MiB) | `27ea653c549c604840f1c676e3d08818ac06019a1a2f353a8ccbb7aabd4edb66` | 11 | `valkey-9.2.0-jammy-x86_64/` |

(`*.sha256` format: `"<hex>  <filename>\n"`, 99 bytes.)

Archive layout (both variants identical structure):

```
valkey-9.2.0-<variant>-x86_64/
  bin/
    valkey-server
    valkey-cli
    valkey-benchmark
    valkey-sentinel
    valkey-check-aof
    valkey-check-rdb
    valkey-benchmark-dataset.h
  share/
    LICENSE
```

No `lib/`, no `conf/`, no bundled shared libraries. Extracted size: **~106–107M** (noble 106M, jammy 107M). **`valkey-cli` is bundled** (9.2.0 git:d23684e3). `valkey-server --version`:
- noble: `Valkey server v=9.2.0 sha=d23684e3:0 malloc=jemalloc-5.3.0 bits=64 build=13d9d38e68294b43`
- jammy: `Valkey server v=9.2.0 sha=d23684e3:0 malloc=jemalloc-5.3.0 bits=64 build=1871b28ae66d905e`

## 2. `ldd`, glibc floor, and **ASSUMPTION-02** (jammy/noble on non-Ubuntu)

Host: glibc **2.43**, Fedora 44. **0 "not found"** for both variants; both started, accepted `PING`, and served traffic successfully on this machine.

`ldd bin/valkey-server` resolves to **host** libraries (not bundled):

```
	linux-vdso.so.1
	libm.so.6        => /lib64/libm.so.6
	libsystemd.so.0  => /lib64/libsystemd.so.0
	libssl.so.3      => /lib64/libssl.so.3
	libcrypto.so.3   => /lib64/libcrypto.so.3
	libc.so.6        => /lib64/libc.so.6
	/lib64/ld-linux-x86-64.so.2
	libgcc_s.so.1    => /lib64/libgcc_s.so.1
	libz.so.1        => /lib64/libz.so.1
```

glibc symbol floors (`readelf -V`, max required symbol):

| Variant | Max `GLIBC_*` required | Extra vs jammy |
|---|---|---|
| **jammy** | **GLIBC_2.34** | — |
| **noble** | **GLIBC_2.38** | `GLIBC_2.38` |

**ASSUMPTION-02 verdict:** **Partially confirmed, with an important qualifier.** The Ubuntu tarballs are dynamically linked to the host (not self-contained) and need `libsystemd.so.0` + `libssl.so.3` + `libcrypto.so.3` present on the host — they ran on Fedora 44 because those libraries exist here. However, the **noble** build requires **glibc >= 2.38** while the **jammy** build requires **glibc >= 2.34**. A non-Ubuntu distro with glibc in `[2.34, 2.38)` (e.g. some older LTS/RHEL-family environments) would run **jammy but not noble**; anything older than 2.34 would run neither. The README/help text's "Ubuntu 22.04+/24.04+" labels are therefore *minimum glibc* constraints in effect, not Ubuntu-only constraints.

**Recommendation:** prefer the **jammy** tarball as the broader-compatibility default for x86_64 (lower glibc floor: 2.34), fall back/offer noble only where appropriate; and at install/start preflight, check for `libsystemd.so.0`, `libssl.so.3` (OpenSSL 3 ABI), `libcrypto.so.3`, and glibc >= the variant's floor. A catalog entry must record the required glibc/soname set per variant.

## 3. Minimal `valkey.conf`, loopback-only

Working config (the spike used this, `daemonize no`, server foregrounded by us and detached with `setsid`):

```conf
bind 127.0.0.1 -::1           # loopback only, IPv4 and IPv6
protected-mode yes
port <41000-41999>
tcp-backlog 511
timeout 0
tcp-keepalive 300
daemonize no                  # DBLocal manages the process; do NOT daemonize
supervised no
pidfile <instance>/run/valkey.pid
loglevel notice
logfile <instance>/logs/valkey.log
databases 16
dir <instance>/data
requirepass <24-char-random>  # D-17
# persistence (choose per product policy):
appendonly yes
appendfilename "appendonly.aof"
appenddirname "appendonlydir"
save 3600 1 300 100 60 10000
# or to disable: save "" ; appendonly no
```

Notes:
- `bind 127.0.0.1 -::1` binds **both** loopback stacks and nothing else. `ss` proof: `LISTEN 127.0.0.1:41300` and `LISTEN [::1]:41300`, **no `0.0.0.0`**. `bind 127.0.0.1` alone would bind IPv4 loopback only.
- `daemonize no` + `supervised no`: the adapter owns the process lifetime (detached via `setsid`/process-group management) and observes exit; `daemonize yes` would fork and complicate pid/wait tracking.
- `protected-mode yes` even with a password and loopback bind (defense in depth); `protected-mode no` still works but is unnecessary.
- `dir` **must exist** before start (see failures). `logfile` directory must be writable.
- AOF (appendonly) creates `<dir>/appendonlydir/` with `appendonly.aof.*.base.rdb`, `*.incr.aof`, `appendonly.aof.manifest`. RDB snapshot file is `dump.rdb` when produced.

## 4. Start detached, time to `PONG`, auth/unauth `PING`

Started detached: `setsid valkey-server <conf> </dev/null &` (disowned). Spawn → `PONG` (authenticated poll): **25–34 ms** across runs. pidfile written with the server PID; `comm=valkey-server`; process in its own session/process group (`sid==pgid==pid` from our spawn).

```
$ valkey-cli -h 127.0.0.1 -p 41300 -a <pw> --no-auth-warning PING
PONG

$ valkey-cli -h 127.0.0.1 -p 41300 PING
NOAUTH Authentication required.
(exit 0 in this client build — see note below)

raw RESP, no auth:  b'-NOAUTH Authentication required.\r\n'
raw RESP, AUTH <pw>\r\nPING\r\n:  b'+OK\r\n+PONG\r\n'
```

**Important client nuance:** in this `valkey-cli` 9.2.0, an unauthenticated `PING` printed `NOAUTH Authentication required.` but exited **0**. The raw RESP is an error (`-NOAUTH...`), and `redis-cli`/client libraries will surface it as an error; do **not** rely on `valkey-cli`'s process exit code alone to detect auth failure — parse the response or check for the `-NOAUTH`/error class.

## 5. Stop modes, timing, persistence, `kill -9`

| Action | Time | Outcome |
|---|---:|---|
| `SHUTDOWN NOSAVE` (client command) | **9–12 ms** | Clean exit, pidfile removed, returns quickly, skips save work. |
| `SHUTDOWN SAVE` (explicit) | **11 ms** (small dataset) | Clean exit, final RDB/AOF state saved; `dump.rdb` present after. |
| `SIGTERM` (no save point reached, AOF on) | **110–112 ms** | **Graceful shutdown**: Valkey handled SIGTERM, logged `Received SIGTERM scheduling shutdown...` / `User requested shutdown...` / `Calling fsync() on the AOF file.` / `Saving the final RDB snapshot before exiting.` / `DB saved on disk` / `Removing the pid file.` and exited cleanly. Data written before SIGTERM survived restart (AOF). |
| `kill -9` (crash) | immediate | pidfile **left stale** on disk. Restart recovered from AOF in **24–27 ms**: `Loading RDB produced by Valkey version 9.2.0` / `RDB is base AOF` / `DB loaded from base file ...` / `DB loaded from incr file ...` / `DB loaded from append only file` / `Ready to accept connections`. Previously-written key (`SET spike:k3 v3`) survived. |

**Recommendation:** prefer `SHUTDOWN NOSAVE` (or `SHUTDOWN SAVE` if a final snapshot is desired) issued via `valkey-cli`/RESP for a controlled stop with a known timeout; escalate to `SIGTERM` then `SIGKILL` only if unresponsive (and record forced-kill in logs). `SIGTERM` is treated gracefully by Valkey (unlike a blind assumption of abrupt kill), but `SHUTDOWN` gives explicit control over save behavior and a clear success signal.

## 6. Failure texts and exit codes (F-LOG-02) — verbatim

All from `valkey-server` 9.2.0 (jammy build). Foreground/config errors generally exit **1**. Startup-failure causes that happen after config parsing are typically written to `logfile` (and may also echo a monotonic-clock note to stderr); **the adapter must read the instance log** to diagnose.

### 6.1 Port already in use

Exit **1**. Log (and stdout when no logfile):
```
<M>:M 20 Nov 2026 20:17:08.351 # Warning: Could not create server TCP listening socket 127.0.0.1:41501: bind: Address already in use
<M>:M 20 Nov 2026 20:17:08.351 # Failed listening on port 41501 (tcp), aborting.
```
(Preceded by normal startup banner lines: `oO0OoO0OoO0Oo Valkey is starting ...`, `Valkey version=...`, `Configuration loaded`, `Running mode=standalone, port=41501.`)  
**Suggested regex:** `Could not create server TCP listening socket (?P<addr>[^:]+:[0-9]+): bind: Address already in use` or `Failed listening on port (?P<port>\d+) .* aborting\.`

### 6.2 Bad config — unknown directive

Exit **1**, stderr only:
```
*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***
Reading the configuration file, at line 10
>>> 'nonexistent_directive_on_purpose 1'
Bad directive or wrong number of arguments
```
**Regex:** `FATAL CONFIG FILE ERROR` / `Bad directive or wrong number of arguments` (line number and offending line are included).

### 6.3 Bad config — wrong value type

Exit **1**:
```
*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***
Reading the configuration file, at line 10
>>> 'maxmemory not-a-number'
argument must be a memory value
```

### 6.4 Bad config — port out of range

Exit **1**:
```
*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***
Reading the configuration file, at line 3
>>> 'port 99999'
argument must be between 0 and 65535 inclusive
```

### 6.5 Bad config — bind to unavailable address

Exit **1**. Log:
```
<M>:M ... # Warning: Could not create server TCP listening socket 10.255.255.1:41502: bind: Cannot assign requested address
<M>:M ... # Failed listening on port 41502 (tcp), aborting.
```
**Regex:** same as 6.1 but match the bind reason generically: `Could not create server TCP listening socket [^:]+:\d+: bind: .*` + `Failed listening on port \d+ .* aborting`.

### 6.6 Missing data directory

Exit **1**, stderr:
```
*** FATAL CONFIG FILE ERROR (Version 9.2.0) ***
Reading the configuration file, at line 7
>>> 'dir /.../does-not-exist'
No such file or directory
```
**Regex:** config-file error block with `No such file or directory` for `dir`.

### 6.7 Unwritable / unusable data dir (append-only)

Exit **1**. Cause appears **only in the logfile** (not on stdout) after "Server initialized":
```
<M>:M ... * Server initialized
<M>:M ... # Can't open or create append-only dir appendonlydir: Permission denied
```
(With a writable dir and `appendonly yes` the server reaches `Ready to accept connections tcp` instead.) A corrupted/occupied appendonly path: `# Can't open or create append-only dir appendonlydir: Not a directory`.  
**Regex:** `Can't open or create append-only dir (?P<name>\S+): (?P<reason>.+)`. **This is a runtime/post-init failure written to the log** — the foreground process exits 1 but the human-actionable reason is in `logfile`, not stderr.

### 6.8 Informational warnings (not failures)

`# WARNING Memory overcommit must be enabled! ...` appears on startup (kernel/sysctl advisory) — benign for local dev, noisy in logs.

## 7. Valkey on `redis://` URIs (and `valkey://`)

Tested with a URI-hostile password `pa@ss:w/rd?#%` (percent-encoded in the URI) and `valkey-cli -u`:

| URI scheme | Result |
|---|---|
| `redis://default:<enc>@127.0.0.1:<port>/0` | **PONG** (rc 0). (cli warns about password on argv/-u.) |
| `valkey://default:<enc>@127.0.0.1:<port>/0` | **PONG** (rc 0). |
| `rediss://default:<enc>@127.0.0.1:<port>/0` | **Failed**: `Could not connect to Valkey at 127.0.0.1:<port>: SSL_connect failed: wrong version number` (TLS not configured on this plain server — expected). |

`valkey-cli -h/-p/-a` and `--user default -a <pw>` both worked (ACL user `default` with `requirepass`). **Conclusion:** both `redis://` and `valkey://` URI schemes work with `valkey-cli` when the password is percent-encoded; `redis://` is the safe interoperability choice for generic Redis clients, and `valkey://` is also accepted by the bundled `valkey-cli`. Passwords with URI-reserved characters **must** be percent-encoded (same rule as PostgreSQL).

## 8. Availability / matrix notes (from probing)

- `download.valkey.io/releases/` serves **x86_64** for noble and jammy (200, with `.sha256`). **`aarch64` in the filename returns 403 (not listed/served at that name)** — the correct architecture token for arm64 tarballs is **`arm64`** (`valkey-9.2.0-noble-arm64.tar.gz` → 200, ~44.3 MiB; `valkey-9.2.0-jammy-arm64.tar.gz` → 200, ~44.5 MiB; `*.sha256` → 200). (No S3 listing is exposed; probed by name.)
- No macOS or Windows binary tarballs/zip assets found at `download.valkey.io` for 9.2.0 (guessed names returned 403). Valkey GitHub releases (9.2.0, 9.1.4, 9.0.8, …) publish **no binary assets** (source tarball/zipball only). Building macOS/Windows binaries remains a spike/CI responsibility (PRD F-SPK-02 / D-13).
- The tarballs are **not self-contained** (host glibc/ssl/crypto/systemd). This is the main portability constraint versus the PostgreSQL `theseus-rs` tarballs, which bundle most deps.

## 9. Suggested adapter outline (Valkey)

- **Preflight:** verify glibc >= variant floor, `libsystemd.so.0` + `libssl.so.3` + `libcrypto.so.3` resolvable, `dir`/log/pid parent dirs exist and writable.
- **Config:** generate a minimal conf (loopback bind `127.0.0.1 -::1`, `protected-mode yes`, `daemonize no`, `pidfile`, `logfile`, `dir`, `requirepass`, explicit persistence choice). Never log the password.
- **Init:** create instance dirs; no initdb-equivalent needed (server initializes on first start).
- **Spawn:** `setsid valkey-server <conf>` detached, stdio to /dev/null (logfile handles logs). Record PID + exe + start time.
- **Health:** poll `valkey-cli -h 127.0.0.1 -p <port> -a <pw> --no-auth-warning PING` expecting `PONG` (parse response, don't trust exit code alone for auth); timeout ~10s.
- **Stop:** `SHUTDOWN NOSAVE` (or `SAVE` per policy) with timeout; escalate SIGTERM → wait → SIGKILL; log forced kill.
- **PID validation:** verify exe basename/path contains `valkey-server`, compare start time (pidfile written by server + `/proc/<pid>/stat` starttime) before signalling (same PID-reuse discipline as Postgres).
- **Diagnosis:** read the tail of `logfile` and match `Failed listening on port`, `Address already in use`, `Cannot assign requested address`, `FATAL CONFIG FILE ERROR`, `Can't open or create append-only dir ...: ...`, `No such file or directory` on `dir`.
- **URI:** `redis://default:<percent-encoded-password>@127.0.0.1:<port>/0` for broad client compatibility (`valkey://` also works with bundled cli). Always percent-encode.
