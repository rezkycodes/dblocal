# MySQL 8.4 LTS spike (W3-B1, Linux x86_64)

Date: 2026-11-22  
Host: Fedora Linux 44, kernel 7.3.0-cachyos1.fc44.x86_64, glibc 2.43, x86_64, 16 cores, no sudo.  
Source: `https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz` (redirected to `cdn.mysql.com//Downloads/MySQL-8.4/...`).  

## Download, verification, size

- Minimal tarball: `mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz`, **79,777,440 bytes** (~76.1 MiB), HTTP 200. Cached at `$HOME/.cache/dblocal-dev/mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz` (outside repo).
- SHA-256 (computed locally): `383f54e124d5f325d67f0c6912a8f96814eedc761a17ea30112e52fa4cc6b143`.
- MD5 (computed locally): `3358302178e35d893f14fc49be542847` (matches upstream sidecar expectation shape; upstream has no SHA-256 sidecar, D-24/C-02).
- GPG `.asc`: `https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.11-linux-glibc2.28-x86_64-minimal.tar.xz.asc` (833 bytes, HTTP 200). Verified with `gpg --verify` in a temp `GNUPGHOME` after importing key `BCA43417C3B485DD128EC6D4B7B3B788A8D3785C` (MySQL Release Engineering <mysql-build@oss.oracle.com>, RSA4096, expires 2027-10-23) from `hkps://keyserver.ubuntu.com`. Result: **Good signature**.

## Layout / strip_prefix

- `strip_prefix`: `mysql-8.4.11-linux-glibc2.28-x86_64-minimal/`
- Top-level: `bin/ docs/ include/ lib/ LICENSE man/ README share/ support-files/`
- `bin/` contains: `ibd2sdi innochecksum myisamchk myisam_ftdump myisamlog myisampack my_print_defaults mysql mysqladmin mysqlbinlog mysqlcheck mysql_config mysql_config_editor mysqld mysqld_multi mysqld_safe mysqldump mysqldumpslow mysqlimport mysql_migrate_keyring mysql_secure_installation mysqlshow mysqlslap mysql_tzinfo_to_sql perror`
- Minimal tarball contains required client/admin binaries for this adapter: `mysqld`, `mysql`, `mysqladmin`. It does not contain `mysqlsh`.
- Sizes: total 405M extracted, `bin` 177M, `lib` 216M, `share` 11M.

## glibc floor / dynamic deps

- `mysqld --version`: `/tmp/.../bin/mysqld  Ver 8.4.11 for Linux on x86_64 (MySQL Community Server - GPL)`
- `mysql --version`: `./bin/mysql  Ver 8.4.11 for Linux on x86_64 (MySQL Community Server - GPL)`
- `readelf -V bin/mysqld | grep GLIBC_ | sort -Vu | tail`: `GLIBC_2.2.5 ... GLIBC_2.17 GLIBC_2.18 GLIBC_2.27 GLIBC_2.28` (floor **GLIBC_2.28**, matches tarball name).
- `ldd bin/mysqld`: **0 "not found"**. Many libs resolved via `RUNPATH: [$ORIGIN/../lib/private]` (libssl/libcrypto/libprotobuf/abseil bundled). System deps include libc/libm/libpthread/librt/libdl/libgcc_s/libstdc++/libresolv.
- `readelf -d bin/mysqld` shows `RUNPATH [$ORIGIN/../lib/private]`.

## initialize-insecure

Command:

```sh
mysqld --no-defaults --initialize-insecure \
  --datadir="$DATA" --basedir="$RT" --log-error="$LOGS/init.err"
```

Observed: rc 0, ~2015 ms, datadir ~190M. Key log lines:

```text
2026-11-21T23:52:37.237178Z 0 [System] [MY-015017] [Server] MySQL Server Initialization - start.
2026-11-21T23:52:37.238278Z 0 [System] [MY-013169] [Server] ... mysqld (mysqld 8.4.11) initializing of server in progress as process 607844
2026-11-21T23:52:37.242226Z 1 [System] [MY-013576] [InnoDB] InnoDB initialization has started.
2026-11-21T23:52:37.346141Z 1 [System] [MY-013577] [InnoDB] InnoDB initialization has ended.
2026-11-21T23:52:37.871933Z 5 [Warning] [MY-010453] [Server] root@localhost is created with an empty password ! Please consider switching off the --initialize-insecure option.
2026-11-21T23:52:39.191690Z 0 [System] [MY-015018] [Server] MySQL Server Initialization - end.
```

Important: `--initialize-insecure` creates only `root@localhost` with an empty password. There is **no** `root@127.0.0.1`. Connecting over TCP as root before adding that user fails with `ERROR 1130 (HY000): Host '127.0.0.1' is not allowed to connect to this MySQL server`.

`--init-file` during `--initialize-insecure` was ignored for credential setup in the obvious way? In our spike it ran but the init-file was still on disk and root remained empty-password localhost-only; do not rely on it. `--init-file` at **normal startup** works (see below).

## Listeners / X Protocol

Config (mysqlx disabled):

```ini
[mysqld]
bind-address=127.0.0.1
port=41100
datadir=...
socket=...
pid-file=...
log-error=...
mysqlx=OFF
skip-name-resolve
basedir=...
```

With `mysqlx=OFF`: `ss -ltnp` showed only `127.0.0.1:41100` for mysqld; no `:33060`, no `mysqlx.sock`. `ss -lxnp` showed only the configured mysqld unix socket.

Without `mysqlx=OFF` (default): `ss -ltnp` showed `127.0.0.1:41102` **and** `*:33060`; `ss -lxnp` showed `/tmp/mysqlx.sock` in addition to the configured socket. This violates the "instance must listen ONLY on its own port at 127.0.0.1" rule, so the adapter always writes `mysqlx=OFF`.

## Socket path limit

Socket path length 43 bytes worked. A conventional socket path of 159 bytes caused:

```text
2026-11-21T23:56:34.517296Z 0 [ERROR] [MY-010267] [Server] The socket file path is too long (> 107): /tmp/.../mysqld.sock
2026-11-21T23:56:34.517313Z 0 [ERROR] [MY-010119] [Server] Aborting
```

The process stayed alive long enough to be confusing in one run but logged abort; the adapter validates the full socket path length at create/start and returns a typed invalid-input error before spawning.

Linux abstract-namespace socket (`socket=@name`) was accepted by mysqld and the server listened; however `mysqladmin -S @name` did not connect via the usual client path in this quick test. The adapter does **not** use abstract sockets; it uses a short conventional socket under `<instance>/run/mysqld-<port>.sock` and rejects over-long paths.

## Bootstrap / credential strategy

Because `root@127.0.0.1` is absent after `--initialize-insecure`, the adapter bootstraps credentials by:

1. `mysqld --initialize-insecure --datadir ... --basedir ... --log-error ...`
2. Write `my.cnf` (0600) with `bind-address=127.0.0.1`, `port`, `datadir`, short `socket`, `pid-file`, `log-error`, `mysqlx=OFF`, `skip-name-resolve`.
3. Write a one-shot init SQL file (0600) in `run/dblocal-init.sql`.
4. Start `mysqld --defaults-file=<my.cnf> --init-file=<init.sql>` (foreground, no password in argv/env).
5. Wait for TCP connect on 127.0.0.1:port.
6. Run `mysqladmin --no-defaults -h 127.0.0.1 -P <port> -u root --protocol=TCP shutdown`, passing password via `MYSQL_PWD` env only when auth is Password.
7. Wait for mysqld exit, then delete the init SQL file. On any failure remove the init SQL file and the partial data directory.

Init SQL for `AuthMode::Password`:

```sql
ALTER USER 'root'@'localhost' IDENTIFIED BY '...';
CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '...';
GRANT ALL PRIVILEGES ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;
FLUSH PRIVILEGES;
```

Init SQL for `AuthMode::None` (F-CRD-03): adds `root@127.0.0.1` with empty password and grants; keeps localhost account as created by `--initialize-insecure`.

Spike verification: `--init-file` on normal startup applied the SQL; TCP login with password succeeded; empty-password TCP login was rejected (1045).

## Shutdown

- `mysqladmin --no-defaults -h 127.0.0.1 -P <port> -u root --protocol=TCP shutdown` with correct password: clean shutdown. Spike bootstrap transient shutdown logged `Received SHUTDOWN from user root...` and `Shutdown complete (mysqld 8.4.11) ...`.
- `SIGTERM` to mysqld: exited cleanly in ~1342 ms. Log:

```text
2026-11-21T23:54:47.719448Z 0 [System] [MY-013172] [Server] Received SHUTDOWN from user <via user signal>. Shutting down mysqld (Version: 8.4.11).
2026-11-21T23:54:48.993682Z 0 [System] [MY-010910] [Server] /tmp/.../mysqld: Shutdown complete (mysqld 8.4.11)  MySQL Community Server - GPL.
2026-11-21T23:54:48.993699Z 0 [System] [MY-015016] [Server] MySQL Server - end.
```

Adapter uses `mysqladmin shutdown` (bounded by command completion; caller/service enforces stop grace/forced kill). SIGTERM is an acceptable fallback per spike; the adapter's current path is SQL shutdown through mysqladmin.

## Health check

`mysqladmin ping` returned rc 0 in one case while also printing `Host '127.0.0.1' is not allowed to connect to this MySQL server` — so exit code is not trustworthy as readiness. The adapter does a real TCP protocol handshake: it reads the 4-byte MySQL packet header and payload. Protocol version `10` greeting means ready. An `ERR` packet with auth/host challenge codes/messages (`1045` Access denied, `1130` Host ... not allowed, `1251`, `1524`) means the server is up and answering the protocol, so it returns `Health::Ready` (same contract as PostgreSQL auth challenge / Valkey NOAUTH).

## Failure texts (verbatim, from spike)

### Port in use

```text
2026-11-21T23:56:30.686109Z 0 [ERROR] [MY-010262] [Server] Can't start server: Bind on TCP/IP port: Address already in use
2026-11-21T23:56:30.686124Z 0 [ERROR] [MY-010257] [Server] Do you already have another mysqld server running on port: 41102 ?
2026-11-21T23:56:30.686149Z 0 [ERROR] [MY-010119] [Server] Aborting
```

Mapped to `StartFailure::PortInUse`.

### Wrong / missing data directory (empty dir, missing ibdata1)

```text
2026-11-21T23:56:32.516016Z 1 [ERROR] [MY-012592] [InnoDB] Operating system error number 2 in a file operation.
2026-11-21T23:56:32.516041Z 1 [ERROR] [MY-012593] [InnoDB] The error means the system cannot find the path specified.
2026-11-21T23:56:32.516047Z 1 [ERROR] [MY-012594] [InnoDB] If you are installing InnoDB, remember that you must create directories yourself, InnoDB does not create them.
2026-11-21T23:56:32.516055Z 1 [ERROR] [MY-012646] [InnoDB] File ./ibdata1: 'open' returned OS error 71. Cannot continue operation
2026-11-21T23:56:32.516068Z 1 [ERROR] [MY-012981] [InnoDB] Cannot continue operation.
```

Mapped to `StartFailure::Unknown` (raw tail kept; no honest "missing binary" or "version mismatch" variant without stronger signal).

### Garbled / incompatible data directory

```text
2026-11-21T23:56:32.767385Z 1 [ERROR] [MY-012263] [InnoDB] The Auto-extending innodb_system data file './ibdata1' is of a different size 0 pages (rounded down to MB) than specified in the .cnf file: initial 768 pages, max 0 (relevant if non-zero) pages!
2026-11-21T23:56:32.767415Z 1 [ERROR] [MY-012930] [InnoDB] Plugin initialization aborted with error Generic error.
2026-11-21T23:56:33.267699Z 1 [ERROR] [MY-010334] [Server] Failed to initialize DD Storage Engine
2026-11-21T23:56:33.268000Z 0 [ERROR] [MY-010020] [Server] Data Dictionary initialization failed.
2026-11-21T23:56:33.268032Z 0 [ERROR] [MY-010119] [Server] Aborting
```

Mapped to `StartFailure::Unknown`.

### Bad configuration (invalid value)

```text
2026-11-21T23:56:33.765723Z 0 [ERROR] [MY-000058] [Server] Unknown suffix 'n' used for variable 'port' (value 'not-a-number').
2026-11-21T23:56:33.765729Z 0 [ERROR] [MY-000077] [Server] /tmp/.../bin/mysqld: Error while setting value 'not-a-number' to 'port'.
2026-11-21T23:56:33.765762Z 0 [ERROR] [MY-010119] [Server] Aborting
```

Mapped to `StartFailure::Unknown` (no dedicated bad-config variant).

### Permission denied

```text
2026-11-21T23:56:34.008997Z 0 [Warning] [MY-010091] [Server] Can't create test file /tmp/w3b1-spike/s3-BDZxvj/ro/mysqld_tmp_file_case_insensitive_test.lower-test
...
mysqld: File './binlog.index' not found (OS errno 13 - Permission denied)
2026-11-21T23:56:34.009506Z 0 [ERROR] [MY-010119] [Server] Aborting
```

Mapped to `StartFailure::PermissionDenied`.

### Socket path too long

```text
2026-11-21T23:56:34.517296Z 0 [ERROR] [MY-010267] [Server] The socket file path is too long (> 107): /tmp/.../mysqld.sock
2026-11-21T23:56:34.517313Z 0 [ERROR] [MY-010119] [Server] Aborting
```

Mapped to `StartFailure::Unknown`; adapter also proactively rejects over-long socket paths with `DblError::InvalidInput { field: "socket", ... }`.

## Caveats / unverified

- macOS, Windows, aarch64, other glibc floors: not verified (logic written, no runtime proof).
- MySQL 9.x: not verified.
- The adapter uses `mysqladmin` for shutdown and bootstrap shutdown; `mysqladmin` is present in the minimal tarball. It passes password via `MYSQL_PWD` env, never argv.
- No `mysql_async` client dependency added; health is a small handshake reader. This keeps the adapter small and avoids a new protocol dependency surface.
- Real-engine e2e in this environment: run under `flock -w 3600 $HOME/.cache/dblocal-e2e.lock` per _COMMON-RULES.md rule 12.