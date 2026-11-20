# W0-B — Engine spikes: PostgreSQL and Valkey on real binaries (Linux)

Branch: `w0-engine-spikes`. Read first: AGENTS.md, then PRD §6 `F-SPK-03`, `F-RTM`, `F-INS-02..04`, `F-LOG-02`,
`F-PRT-02` and decisions D-12, D-13, D-16, D-17.

## Goal

The Wave 1 engine adapter worker will code against what you find here. Prove, with real binaries on this
machine, how PostgreSQL and Valkey are downloaded, laid out, initialised, started, health-checked, stopped
and diagnosed, and record **exact commands, flags, timings and error texts**. You write no Rust. Scripts are POSIX sh or bash.

## You own

`spikes/engines/**`, `docs/spikes/engines/**`. Nothing else.

## Safety rules (violating these is a failure)

- Use ports **41000-41999 only**. Never 5432 or 6379 (the user may have system servers there).
- Work in `mktemp -d` directories. Never touch `~/.dblocal`. Cache downloads in `$HOME/.cache/dblocal-dev/`.
- Every started process is stopped and every data dir removed when a script ends (use `trap`). Report any leftover.
- Do not use sudo.

## Deliverables

### 1. PostgreSQL (`docs/spikes/engines/postgres.md` + `spikes/engines/postgres/*.sh`)

Source: GitHub releases of `theseus-rs/postgresql-binaries` (asset `postgresql-<ver>-x86_64-unknown-linux-gnu.tar.gz`
with a `.sha256` file). Use the latest 18.x **and** one older major (17.x or 16.x).

Answer, with evidence for each:

- Download, SHA-256 verification, archive layout (top-level directory name, so the catalog `strip_prefix` is exact), size.
- `ldd` on `bin/postgres`: any missing shared library on this machine? Is the tarball self-contained?
- `initdb` flags that work: UTF-8 encoding, locale (`C.UTF-8` or a fallback), auth modes. Password auth
  (`--pwfile`, `scram-sha-256`) and `trust`. Time taken.
- **Unix socket directory.** The default may be `/tmp` or `/var/run/postgresql`. Prove how to point it at a per-instance
  directory, and the 107-byte socket path limit: what happens with a long home path.
- Config to force `listen_addresses = '127.0.0.1'` (and `::1`?). Prove nothing listens on `0.0.0.0` using `ss -ltnp`.
- Start: `pg_ctl ... start` versus running `postgres` directly, detached. Which survives the parent shell exiting?
  Which gives a usable pidfile? Time from start to accepting connections.
- Health check: `pg_isready`, and a real protocol handshake. Is a client such as `psql` bundled in the tarball?
- **Two majors side by side** on different ports, each queried with `SELECT version()`. This proves the core value (D-03).
- Stop: `pg_ctl stop -m fast` timing; behaviour with open connections; behaviour after `kill -9` (stale `postmaster.pid`, crash recovery time).
- `postmaster.pid`: document every line. Can it back the PID validation rule (executable name + start time)?
- **Failure texts for diagnosis (`F-LOG-02`)**, verbatim stderr and exit code for: port already in use; data dir created by
  another major version; data dir permission denied; missing binary. Give a regex per failure that the adapter can match.

### 2. Valkey (`docs/spikes/engines/valkey.md` + `spikes/engines/valkey/*.sh`)

Source: `https://download.valkey.io/releases/` (`valkey-<ver>-noble-x86_64.tar.gz` and `jammy`, with `.sha256`). Latest 9.x.

- Layout, `strip_prefix`, size, `ldd`. **Does the Ubuntu `jammy`/`noble` tarball run on this non-Ubuntu machine?** (ASSUMPTION-02.)
  Try both; record glibc version here and what each needs.
- Minimal `valkey.conf`: `bind 127.0.0.1`, `port`, `dir`, `pidfile`, `logfile`, `daemonize no`, `protected-mode yes`,
  `requirepass`, persistence settings. Is `valkey-cli` bundled?
- Start detached; time to `PONG`; authenticated and unauthenticated `PING`.
- Stop: `SHUTDOWN NOSAVE` versus SIGTERM, timing, data persisted or not. Behaviour after `kill -9`.
- Failure texts and regexes for port in use and bad config.
- Valkey on `redis://` URIs: confirm `redis-cli`-style and `valkey-cli` URIs both work with a password.

### 3. Upstream source table (`docs/spikes/engines/upstream-sources.md`), metadata only

For **PostgreSQL, MySQL 8.4 LTS, MariaDB (latest LTS), Valkey** across linux/macos/windows × x86_64/aarch64, give the exact
download URL pattern, where the SHA-256 comes from, archive format, approximate size, and whether it exists at all.
Use HTTP `HEAD` and the vendors' APIs (`downloads.mariadb.org/rest-api`, GitHub API). **Do not download** MySQL, MariaDB or
macOS/Windows archives. Mark every missing combination clearly. The catalog worker (W1-B) builds directly from this table.

### 4. Summary (`docs/spikes/engines/README.md`)

A one-page table of findings, **every place the PRD's assumptions were wrong or incomplete**, and a recommended
`EngineAdapter` implementation outline for PostgreSQL and Valkey (init args, spawn command, health check, shutdown, PID validation).

## Evidence required in your report

Paste the key command output for each answered question, the final `ss -ltnp` and `ps` showing no leftovers, and the
list of files you produced. Findings that contradict the PRD go first.
