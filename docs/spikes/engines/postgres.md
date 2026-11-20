# PostgreSQL Engine Spike (Linux x86_64)

Date: 2026-11-20  
Machine: Fedora Linux 44 (Workstation Edition), kernel `7.3.0-cachyos1.fc44.x86_64`, glibc **2.43**, user `rezkycodes` (uid 1000).  
Scope: PRD F-SPK-03, F-RTM, F-INS-02..04, F-LOG-02, F-PRT-02; decisions D-12, D-13, D-16, D-17.  
Safety: ports **41000-41999 only** (never 5432), all work in `mktemp -d`, archives cached in `$HOME/.cache/dblocal-dev/`, every started process stopped and temp dirs removed via `trap`, no sudo.

Reproducible scripts (run from repo root):

```sh
bash spikes/engines/postgres/01_download_layout.sh   # download, SHA-256, layout, ldd
bash spikes/engines/postgres/02_initdb.sh            # initdb, auth, locales, unix socket + 107B limit
bash spikes/engines/postgres/03_lifecycle.sh         # start modes, health, two majors, stop, kill -9, pidfile
bash spikes/engines/postgres/04_failures.sh          # verbatim failure texts for F-LOG-02
bash spikes/engines/postgres/05_auth_pid.sh          # scram verifier, URI encoding, detach survival, PID validation
bash spikes/engines/postgres/06_pid_edge.sh          # stale pid w/ live PID reuse, stop timings, edge cases
```

---

## 1. Download, SHA-256 verification, layout, size

Source: GitHub releases of `theseus-rs/postgresql-binaries`. Assets used:

| Major | Version | Asset | Archive bytes | SHA-256 (verified) | Entries |
|---|---|---|---:|---|---:|
| 18 | 18.7.0 | `postgresql-18.7.0-x86_64-unknown-linux-gnu.tar.gz` | 40,530,262 (38.65 MiB) | `53ca01952cb581145e4fb16b6b3b675da345a82879f350707b9f65863a0642f4` | 2895 |
| 17 | 17.12.0 | `postgresql-17.12.0-x86_64-unknown-linux-gnu.tar.gz` | 40,118,562 (38.25 MiB) | `af316b6e42266ec0a153bdc9194e0b1b5f12c9ce4f0b13d9a96dfca77d9aaecc` | 2856 |

Checksum source: the per-asset `*.tar.gz.sha256` file published alongside the asset (format: `"<hex>  <filename>\n"`). Both matched on download.

Archive layout (both majors):

```
postgresql-<ver>-x86_64-unknown-linux-gnu/
  bin/                     (postgres, initdb, pg_ctl, pg_isready, psql, pg_dump, ...)
  include/
  share/
  lib/                     (bundled shared libraries)
  dependency-notices/
  runtime-dependencies.json
  target-validation.json
  source-input.json
  build-manifest.json
  LICENSE COPYRIGHT README.md
```

- **`strip_prefix` (exact, for the catalog):** `postgresql-18.7.0-x86_64-unknown-linux-gnu/` and `postgresql-17.12.0-x86_64-unknown-linux-gnu/` respectively (the single top-level directory name, trailing slash optional depending on extractor). With this stripped, `bin/` is exactly one level inside the runtime root.
- Extracted size (18.7.0): **116M** total — `bin/` 19M, `lib/` 85M, `include/` 8.3M, `share/` 4.1M, `dependency-notices/` 648K. (17.x is comparable.)
- Bundled tooling confirmed in `bin/`: `postgres`, `initdb`, `pg_ctl`, `pg_isready`, `psql`, `createdb`, `createuser`, `dropdb`, `dropuser`, `pg_dump`, `pg_restore`, `pg_basebackup`, `pg_upgrade`, `ecpg`, etc. **`psql` and `pg_isready` are bundled** in the tarball.

Key metadata from `runtime-dependencies.json` / `target-validation.json` (18.7.0): built on Debian 12 (bookworm) packages — `libicu72:amd64 72.1-3+deb12u1`, `libssl3 3.0.22-1~deb12u2`, `liblz4-1 1.9.4-1`, `zlib1g 1:1.2.13.dfsg-1`, `libxml2 2.9.14+dfsg-1.3~deb12u6`, krb5, etc. Target id `x86_64-unknown-linux-gnu`, interpreter `/lib64/ld-linux-x86-64.so.2`, `cflags -O2 -march=x86-64`, little-endian 64-bit.

## 2. `ldd` / self-containedness

`ldd bin/postgres` (18.7.0 and 17.12.0) reported **0 "not found"** libraries on this Fedora 44 (glibc 2.43) machine.

18.7.0 (representative):

```
	linux-vdso.so.1
	libzstd.so.1    => <...>/bin/../lib/libzstd.so.1
	liblz4.so.1     => <...>/bin/../lib/liblz4.so.1
	libxml2.so.2    => <...>/bin/../lib/libxml2.so.2
	libssl.so.3     => <...>/bin/../lib/libssl.so.3
	libcrypto.so.3  => <...>/bin/../lib/libcrypto.so.3
	libgssapi_krb5.so.2 => <...>/bin/../lib/libgssapi_krb5.so.2
	libz.so.1       => <...>/bin/../lib/libz.so.1
	libm.so.6       => /lib64/libm.so.6
	libicui18n.so.72 => <...>/bin/../lib/libicui18n.so.72
	libicuuc.so.72  => <...>/bin/../lib/libicuuc.so.72
	libc.so.6       => /lib64/libc.so.6
	liblzma.so.5    => <...>/bin/../lib/liblzma.so.5
	libkrb5.so.3    => <...>/bin/../lib/libkrb5.so.3
	libk5crypto.so.3 => <...>/bin/../lib/libk5crypto.so.3
	libcom_err.so.2 => <...>/bin/../lib/libcom_err.so.2
	libkrb5support.so.0 => <...>/bin/../lib/libkrb5support.so.0
	/lib64/ld-linux-x86-64.so.2
	libstdc++.so.6  => <...>/bin/../lib/libstdc++.so.6
	libgcc_s.so.1   => <...>/bin/../lib/libgcc_s.so.1
	libicudata.so.72 => <...>/bin/../lib/libicudata.so.72
	libkeyutils.so.1 => <...>/bin/../lib/libkeyutils.so.1
	libresolv.so.2  => /lib64/libresolv.so.2
```

All feature libraries (ICU, SSL/crypto, libxml2, krb5/gssapi, compression, C++/gcc runtime) resolve to the bundled `lib/` via `$ORIGIN/../lib` RPATH-equivalent; only the base system ABI comes from the host (`libc.so.6`, `libm.so.6`, `libresolv.so.2`, `ld-linux-x86-64.so.2`, `linux-vdso`). **Effectively self-contained** for a glibc Linux that meets the symbol floor.

**glibc symbol floor (18.7.0, `readelf -V bin/postgres`, max):** `GLIBC_2.34`. (Host here is 2.43, so it runs.) This means the `x86_64-unknown-linux-gnu` tarball requires a reasonably modern glibc (>= 2.34); the `musl` variant exists for static-ish/older-distro cases but was not exercised.

## 3. `initdb` flags, encoding, locale, auth

Tested with 18.7.0 (`02_initdb.sh`, `05_auth_pid.sh`).

### Working flags

```sh
initdb -D "$DATA" \
  -U postgres \
  --encoding=UTF8 \
  --locale=C.UTF-8 \
  --auth-local=trust \
  --auth-host=trust

initdb -D "$DATA" \
  -U postgres \
  --encoding=UTF8 \
  --locale=C.UTF-8 \
  --auth-local=scram-sha-256 \
  --auth-host=scram-sha-256 \
  --pwfile="$PW_FILE"          # mode 0600, one password line

# mixed (what DBLocal will likely use for local UX + TCP security)
initdb ... --auth-local=trust --auth-host=scram-sha-256 --pwfile="$PW_FILE"
```

- **Encoding:** `--encoding=UTF8` works. `pg_controldata` reports `Data page checksum version: 1` (checksums enabled by default in recent initdb).
- **Locale fallback on this machine:** `locale -a` includes `C`, `C.utf8`, `POSIX`, `en_US.UTF-8`. All of `C.UTF-8`, `C.utf8`, `en_US.UTF-8`, `C`, `POSIX` succeeded with `--encoding=UTF8`. An unsupported locale fails: `initdb: error: invalid locale name "xx_YY.UTF-8"` (hint suggests `--icu-locale`). **Recommendation:** prefer `C.UTF-8`, fall back to `C.utf8`, then `C` (note: `C`/`POSIX` are not UTF-8-collation locales even with UTF-8 encoding — `C.UTF-8`/`C.utf8` are the right UTF-8 choices).
- **Timings (this machine):** trust initdb **~927 ms** (cold extract context) / **~514 ms**; scram initdb **~514 ms** / **~519–1401 ms** depending on fs state. Data dir **~40M** immediately after initdb.

### Auth modes produced (`pg_hba.conf`, non-comment lines)

trust:
```
local   all             all                                     trust
host    all             all             127.0.0.1/32            trust
host    all             all             ::1/128                 trust
local   replication     all                                     trust
host    replication     all             127.0.0.1/32            trust
host    replication     all             ::1/128                 trust
```
scram:
```
local   all             all                                     scram-sha-256
host    all             all             127.0.0.1/32            scram-sha-256
host    all             all             ::1/128                 scram-sha-256
local   replication     all                                     scram-sha-256
host    replication     all             127.0.0.1/32            scram-sha-256
host    replication     all             ::1/128                 scram-sha-256
```
mixed `--auth-local=trust --auth-host=scram-sha-256` produced the expected split: `local ... trust`, `host ... scram-sha-256` (including replication). Verified: unix-socket psql connects without password; TCP without password fails `fe_sendauth: no password supplied`; TCP with correct password succeeds.

### Password storage (SCRAM)

`--pwfile` read fine at mode 0600. Verifier in `pg_authid.rolpassword` is **`SCRAM-SHA-256$4096:<salt>$<stored_key>:<server_key>`** (prefix `SCRAM-SHA-256$`, 4096 iterations observed). Plaintext password was **not** found anywhere in the data directory (0 hits grepping the full datadir). Wrong password: `FATAL:  password authentication failed for user "postgres"` (psql exit 2). Missing password (non-interactive TCP): `fe_sendauth: no password supplied` (exit 2). Passwords must be supplied via `PGPASSWORD`/`.pgpass`/interactive prompt, **never argv**.

### Connection URI / percent-encoding

URIs must percent-encode the password. For a password containing `@ : / ? # % [ ] { } space ' "`, the encoded URI `postgresql://postgres:<encoded>@127.0.0.1:<port>/postgres` connected successfully; the unencoded form failed to parse: `psql: error: missing key/value separator "=" in URI query parameter: "#%[]{} '"x@127.0.0.1:41210/postgres"`.

## 4. Unix socket directory and 107-byte AF_UNIX path limit

- **Default (fresh initdb, `unix_socket_directories` left commented):** postmaster.pid line 5 is `/tmp`; sockets created as `/tmp/.s.PGSQL.<port>` and `/tmp/.s.PGSQL.<port>.lock`. `current_setting('unix_socket_directories')` returns `/tmp`. The shipped `postgresql.conf` documents `#unix_socket_directories = '/tmp'`.
- **Per-instance directory works:** set `unix_socket_directories = '/path/to/instance/run'` (or a comma list). With a 47-byte socket path, postgres started and `psql -h "$SOCKDIR" -p $PORT` worked.
- **107-byte `sun_path` limit — proven:**
  - Socket path `148` bytes (`/tmp/dblocal-pg-init-…/llllllll…llll/sock/.s.PGSQL.41000`, dir 133 bytes): `pg_ctl start` failed (exit 1).
    - `pg_ctl`: `waiting for server to start.... stopped waiting` / `pg_ctl: could not start server`
    - server log: `LOG:  Unix-domain socket path ".../.s.PGSQL.41000" is too long (maximum 107 bytes)` / `WARNING:  could not create Unix-domain socket in directory "..."` / `FATAL:  could not create any Unix-domain sockets`
  - Socket path `47` bytes: started fine.
- **Multiple socket dirs:** `unix_socket_directories = '<long-dir>,<short-dir>'` — postgres warned and skipped the too-long dir, then bound the socket in the short dir (`LOG: Unix-domain socket path ... is too long ...` / `WARNING: could not create Unix-domain socket ...` / `LOG: listening on Unix socket "<short-dir>/.s.PGSQL.<port>"`). postmaster.pid line 5 reflected the effective dir list; `psql -h <short-dir>` worked.
- **Missing socket dir:** `FATAL:  could not create lock file "/.../s5/.s.PGSQL.41130.lock": No such file or directory` (and the server did not become ready). The directory **must exist and be writable** before start.

**Implication:** for users whose `~/.dblocal` path is deep, the unix socket cannot live under a long per-instance path. Use a short per-instance socket dir (e.g. a hashed short path under `/tmp`/`$XDG_RUNTIME_DIR` or a dedicated short root) and/or disable unix sockets and rely on TCP loopback. DBLocal must enforce the 107-byte limit at create/start time.

## 5. Listen addresses, `ss` proof (no `0.0.0.0`)

Configured: `listen_addresses = '127.0.0.1'`, `port = 41010`.

```
$ ss -ltnp | grep ':41010'
LISTEN 0 200 127.0.0.1:41010 0.0.0.0:* users:(("postgres",pid=3808775,fd=6))

local addresses: 127.0.0.1:41010
OK: no listener on 0.0.0.0
note: no explicit [::1] line (127.0.0.1 only as configured)
```

With `listen_addresses = 'localhost'` (hostname, resolves to both stacks on this host): listeners on **both** `127.0.0.1:<port>` and `[::1]:<port>`, none on wildcard — but note this depends on name resolution. With `listen_addresses = '::1'` only: IPv6 loopback only, `psql -h ::1` succeeded. **Recommendation:** set `listen_addresses = '127.0.0.1'` (and add `::1` explicitly as `127.0.0.1,::1` only if IPv6 loopback is required and tested), and assert with `ss` that no `0.0.0.0`/`[::]` local address is bound (check the **local** address column, not the peer column).

## 6. Start modes, detached survival, pidfile, accept time

### `pg_ctl -D ... -l ... -w -t 30 start`

- `pg_ctl start wall ms`: **110 ms** (return of `pg_ctl -w`), `pg_ctl->accepting ms` (pg_isready-ready): **128–131 ms** across runs. `-w` waited for ready as advertised.
- postmaster.pid written (8 lines). PID in file matched the live process.
- Process: `comm=postgres`, `ppid=1531` (session leader/reparented context observed), `sid=<pid>` (its own session leader in the run captured), starttime ticks recorded.

### Direct `postgres -D ...` detached (`setsid ... </dev/null &`, disowned)

- Spawn-to-accepting: **78 ms** (both runs identical).
- postmaster.pid present, PID matched the spawned PID. `comm=postgres`, ppid/sid showed an independent session (`setsid`). **Survived parent shell exit** (started with `setsid`, independent session).

### Detached survival across parent-shell exit (explicit test)

- `bash -c "pg_ctl ... start; exit"`: **SURVIVED** — postmaster remained (reparented, own session). `pg_ctl start` backgrounds the postmaster.
- `bash -c "postgres -D ... & exit"` (plain background, no setsid, no pg_ctl): **SURVIVED** in this run (ppid became 1531/reparented). Do **not** rely on this; the robust way is `pg_ctl start` or an explicit `setsid`/double-fork with stdio detached and SIGHUP handled — `pg_ctl` is the supported detached launcher and also gives `-w` readiness.

**Recommendation:** use `pg_ctl start -w -t <timeout> -l <log>` for a usable pidfile + readiness wait. A direct `postgres` process also writes `postmaster.pid` and can be managed, but `pg_ctl` is the safer, documented control surface.

## 7. Health checks

- **`pg_isready` (bundled):** TCP `127.0.0.1:<port> - accepting connections` (rc 0); unix socket `<sockdir>:<port> - accepting connections` (rc 0). `psql` is bundled (`psql (PostgreSQL) 18.7`), so a real protocol handshake is trivial: `psql -h 127.0.0.1 -p <port> -U postgres -d postgres -Atc 'select 1'` (rc 0) and `select current_setting('server_version'), current_setting('listen_addresses')` returned `18.7|127.0.0.1`.
- **`pg_isready` exit codes (observed):**
  - server not running / no response: **rc 2** (`127.0.0.1:41150 - no response`), including with `-t 1` and bogus host.
  - server accepting, valid db: **rc 0**.
  - server accepting, **nonexistent db (`-d nosuchdb`)**: **rc 0** in this build (`127.0.0.1:41150 - accepting connections`). It does not fully authenticate or open the target database in the way `psql` does. A nologin role also returned rc 0.
- **`psql` against a nonexistent db** (server up): `FATAL:  database "nosuchdb" does not exist` (rc 2).

**Implication:** `pg_isready` is a liveness/listen/readiness-to-accept probe, not a "this db+role can log in" probe and, in this build, not a db-existence probe. A true health check that proves the protocol and the target db should do a real handshake/query (e.g. `psql -Atc 'select 1'` or a `tokio-postgres` simple query). `pg_isready` is still useful as a fast pre-check.

## 8. Two majors side by side (core value, D-03)

PG 18.7.0 on **41040** and PG 17.12.0 on **41041**, running concurrently:

```
$ ss -ltnp | grep -E ':(41040|41041)'
LISTEN ... 127.0.0.1:41040 ... users:(("postgres",pid=3809378,...))
LISTEN ... 127.0.0.1:41041 ... users:(("postgres",pid=3809390,...))

$ psql -h 127.0.0.1 -p 41040 -U postgres -Atc 'select version();'
PostgreSQL 18.7 on x86_64-pc-linux-gnu, compiled by gcc (Debian 12.2.0-14+deb12u1) 12.2.0, 64-bit
$ psql -h 127.0.0.1 -p 41040 -U postgres -Atc 'show server_version;'
18.7

$ psql -h 127.0.0.1 -p 41041 -U postgres -Atc 'select version();'
PostgreSQL 17.12 on x86_64-pc-linux-gnu, compiled by gcc (Debian 12.2.0-14+deb12u1) 12.2.0, 64-bit
$ psql -h 127.0.0.1 -p 41041 -U postgres -Atc 'show server_version;'
17.12

concurrent:
pg17-41041|2026-11-20 20:05:56.655052+08
pg18-41040|2026-11-20 20:05:56.546743+08
```

Works: different runtime trees, different data dirs, different ports and socket dirs. This proves the core multi-version value.

## 9. Stop, open connections, `kill -9`, stale pid, crash recovery

- **`pg_ctl stop -m fast` (idle):** **103–116 ms** (also smart/immediate similar when idle: 103–122 ms across runs).
- **`-m fast` with an open connection** (a `psql` running `pg_sleep(300)`): stop completed in **104 ms**. The client saw: `FATAL:  terminating connection due to administrator command` / `server closed the connection unexpectedly` / `This probably means the server terminated abnormally` / `before or while processing the request.` / `connection to server was lost` (psql exit 2).
- **`kill -9` postmaster** after a committed write (`create table t; insert 1000 rows`): process gone immediately; **`postmaster.pid` left on disk unchanged** (stale, 8 lines, still `ready` on line 8). `pg_ctl status` reported `pg_ctl: no server running` (rc 3) because the PID was dead.
  - **Restart with the stale pidfile still present:** `pg_ctl: another server might be running; trying to start server anyway` / `waiting for server to start.... done` / `server started`. Total **112–115 ms**; data recovered (`select count(*) from t` returned `1000`).
  - Recovery log: `LOG:  starting PostgreSQL 18.7 ...` / `LOG:  database system was interrupted; last known up at ...` / `LOG:  database system was not properly shut down; automatic recovery in progress` / `LOG:  checkpoint starting: end-of-recovery immediate wait` / `LOG:  database system is ready to accept connections`.

## 10. `postmaster.pid` format and PID validation backing

Live file (8 lines):

```
     1	3810050                          # PID of the postmaster process
     2	/tmp/.../d6                      # data directory path
     3	1795176359                       # postmaster start timestamp (epoch seconds)
     4	41060                            # port number
     5	/tmp/.../s6                      # unix socket directory (comma list possible)
     6	127.0.0.1                        # listen_addresses token
     7	  2558019     32796              # shared memory key / shmem id (platform-specific)
     8	ready                            # status line
```

Cross-checks:
- `readlink /proc/<pid>/exe` → `<runtime>/bin/postgres`
- `/proc/<pid>/comm` → `postgres`
- `/proc/<pid>/stat` field 22 (starttime ticks): `24962216`, system `btime` `1794926737`, `CLK_TCK` 100 → derived start epoch `1795176359`, **identical to line 3** (delta 0). Line 3 is the postmaster start time in epoch seconds; it validates start time without parsing `/proc/stat` arithmetic (though cross-checking both is stronger).
- Line 8 transitions observed: `ready` while running, briefly `stopping` during shutdown, file **removed on clean shutdown**.

**PID validation (D-17 / F-INS-04 backing):** `pg_ctl status` and a naive "pid alive" check are **unsafe against PID reuse**. Fabricating a pidfile whose line 1 pointed at a live unrelated `sleep` process made `pg_ctl status` report `server is running (PID: <sleep>)` (rc 0) and `pg_ctl start` refused with "another server might be running" while **never signalling** the unrelated process in this case — but an adapter that sends signals by PID alone would target the wrong process. **A correct adapter must, before signalling:** (a) confirm `/proc/<pid>/exe` (or equivalent) resolves to the expected `postgres` binary/path, (b) confirm process identity matches start time (compare `/proc/<pid>/stat` starttime-derived epoch or a recorded start time to pidfile line 3 / recorded value), (c) confirm `comm`/cmdline and ideally data-dir/port context, and (d) treat a mismatch as a stale pidfile (do not signal). Line 3 gives a cheap, portable-within-host start-time token recorded by postgres itself.

**Stale pidfile with dead PID:** `pg_ctl status` returned rc 3 ("no server running"); `pg_ctl start` proceeded after warning "another server might be running; trying to start server anyway" and succeeded (crash recovery path).

## 11. Failure texts for diagnosis (F-LOG-02) — verbatim

All below are from 18.7.0 unless noted.

### 11.1 Port already in use

`pg_ctl start` (exit **1**): `pg_ctl: could not start server` (after "waiting... stopped waiting"). Server log / direct `postgres` stderr (exit **1**):
```
LOG:  starting PostgreSQL 18.7 on x86_64-pc-linux-gnu, compiled by gcc (Debian 12.2.0-14+deb12u1) 12.2.0, 64-bit
LOG:  could not bind IPv4 address "127.0.0.1": Address already in use
HINT:  Is another postmaster already running on port 41100? If not, wait a few seconds and retry.
WARNING:  could not create listen socket for "127.0.0.1"
FATAL:  could not create any TCP/IP sockets
LOG:  database system is shut down
```
**Suggested regex:** `could not bind IPv4 address .*: Address already in use` or `Is another postmaster already running on port (?P<port>\d+)\?` or `FATAL:\s+could not create any TCP/IP sockets`.

### 11.2 Data dir created by another major version

PG 18 `pg_ctl` against a PG 17 datadir: exit **1**, `pg_ctl` printed `pg_ctl: control file appears to be corrupt` (after "waiting"). Server log:
```
FATAL:  database files are incompatible with server
DETAIL:  The data directory was initialized by PostgreSQL version 17, which is not compatible with this version 18.7.
```
**Suggested regex:** `FATAL:\s+database files are incompatible with server` and/or `DETAIL:\s+The data directory was initialized by PostgreSQL version (?P<have>\d+), which is not compatible with this version (?P<want>[0-9.]+)`.

### 11.3 Data dir permission denied

Datadir mode `000`: `pg_ctl` exit **1**, stderr `pg_ctl: could not open PID file "/.../postmaster.pid": Permission denied` (log file itself unwritable, so no postgres log line).  
Datadir mode `500` (read/execute, not writable): server log `FATAL:  could not create lock file "postmaster.pid": Permission denied` (pg_ctl exit 1, "could not start server").  
**Suggested regex:** `Permission denied` on `postmaster.pid` / `lock file` / `PID file` (include the path when present): `could not (?:open|create) (?:PID|lock) file ".*postmaster\.pid": Permission denied`.

### 11.4 Missing / corrupt binary

- `pg_ctl` with a nonexistent data dir argument: `pg_ctl: directory "/.../nodir" does not exist` (exit 1).
- Exec of a nonexistent binary path: shell `No such file or directory`, exit **127**.
- **Corrupt/empty `postgres` binary:** executing an empty file yielded exit **0** with no output in this environment (the kernel/loader behaviour can vary by platform/filesystem; an adapter must not treat exit 0-without-ready as success — always require health-check readiness). A truncated-but-non-empty ELF would typically produce `Exec format error` (ENOEXEC, exit 126 from shell) or a loader error; treat any non-ready start within timeout and any exec failure as "runtime broken, reinstall".
- **Suggested regexes:** `directory ".*" does not exist` (pg_ctl), shell/OS `No such file or directory` / `Exec format error` / `Permission denied` on exec, and a catch-all "process exited before becoming ready" when the binary runs but no readiness signal arrives.

### 11.5 Other captured failures (useful for F-LOG-02)

| Case | Exit | Verbatim (key lines) | Regex |
|---|---:|---|---|
| Unix socket path > 107 bytes | 1 | `LOG:  Unix-domain socket path "..." is too long (maximum 107 bytes)` / `FATAL:  could not create any Unix-domain sockets` | `socket path .* is too long \(maximum 107 bytes\)` |
| Socket dir missing | 1 | `FATAL:  could not create lock file "…/.s.PGSQL.<port>.lock": No such file or directory` | `could not create lock file ".*\.s\.PGSQL\.\d+\.lock": No such file or directory` |
| Datadir is a regular file | 1 | `pg_ctl: could not open PID file "…/notadir/postmaster.pid": Not a directory` / `initdb: error: could not access directory "...": Not a directory` | `Not a directory` (with path) |
| initdb on non-empty/existing dir | 1 | `initdb: error: directory "..." exists but is not empty` + hint | `initdb: error: directory ".*" exists but is not empty` |
| initdb bad locale | 1 | `initdb: error: invalid locale name "xx_YY.UTF-8"` | `initdb: error: invalid locale name` |
| Bad/missing password (TCP, scram) | psql 2 | `FATAL:  password authentication failed for user "postgres"`; `fe_sendauth: no password supplied` | `password authentication failed`, `no password supplied` |
| DB does not exist | psql 2 | `FATAL:  database "nosuchdb" does not exist` | `database ".*" does not exist` |
| Fast stop terminated clients | psql 2 | `FATAL:  terminating connection due to administrator command` | `terminating connection due to administrator command` |
| Data dir path is long/valid but socket dir missing | 1 | see missing socket dir | — |

## 12. Notable nuances

- `pg_isready -d nosuchdb` returned **rc 0** ("accepting connections") in this build even though connecting to that db fails — it is a readiness-to-accept probe, not a target-db probe.
- `pg_ctl stop -m immediate` is similar in wall time to `fast` when idle here (~122 ms) but performs crash-recovery semantics on next start; prefer `fast` for normal shutdown (PRD already states this).
- With `listen_addresses = '127.0.0.1'` only, postgres did **not** bind `[::1]` — so `localhost` may fail on dual-stack environments unless `::1` is added. Be explicit and consistent (`127.0.0.1,::1` or just `127.0.0.1`) and match the health-check host.
- Mixed auth `--auth-local=trust --auth-host=scram-sha-256` is a good UX/security split for local tools (unix socket passwordless, TCP authenticated); DBLocal's "no password" mode maps to trust on both, "password" mode can use scram on host and optionally trust/scram on local per policy.
- `postmaster.pid` line 8 is `ready`/`stopping` and the file is removed on clean stop — but **not** removed on `kill -9`. A stale file is normal after a crash.
- The tarball bundles ICU 72 data/libs (85M in `lib/`, including `libicudata.so.72`), which is why it is large but self-contained.

---

## Evidence index

- Download/layout/ldd/versions: `01_download_layout.sh` output (sizes, sha256, strip_prefix, 0 missing libs, bundled `lib/`).
- initdb/auth/locales/socket-107B: `02_initdb.sh`, `05_auth_pid.sh`.
- listen/ss, start modes, health, side-by-side, stop/kill-9/recovery, pidfile format: `03_lifecycle.sh`.
- verbatim failures + exit codes + pg_isready codes: `04_failures.sh`.
- PID-reuse safety, stop timings, long-socket-in-multi-dir, ::1-only, mixed auth: `06_pid_edge.sh`.
- glibc floor (GLIBC_2.34 max): `readelf -V` on bundled `bin/postgres`.
