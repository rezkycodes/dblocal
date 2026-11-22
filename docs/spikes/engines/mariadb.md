# MariaDB spike (W3-B2, Linux x86_64)

Date: 2026-11-22. Machine: Fedora 44, glibc 2.43, no sudo. All work under `/tmp/w3b2/` and cache in `$HOME/.cache/dblocal-dev/`; ports 43118-43127 only; every server stopped and temp dirs removed.

## Source

MariaDB REST API: `https://downloads.mariadb.org/rest-api/mariadb/` reports 11.8 and 11.4 as LTS. Tested tarballs (SHA-256 verified against REST `checksum.sha256sum`):

| Version | File | SHA-256 | Size |
|---|---:|---|---:|
| 11.8.10 (LTS) | `mariadb-11.8.10-linux-systemd-x86_64.tar.gz` | `15ca40274597af84fa8f89f8248b37740c5a712c003df80b4c7b32a4486e2027` | 413068258 |
| 11.4.14 (LTS) | `mariadb-11.4.14-linux-systemd-x86_64.tar.gz` | `6fa13ea11d07a53ae4795c8c3736dc5aa99c2ef22ece754cff07363dc1faba0d` | 359741460 |

Verification: `sha256sum` on the downloaded file matched the REST API value (pasted in the worker log). The file was downloaded via the REST `file_download_url` HTTPS endpoint.

## Layout / strip_prefix

Top-level dir: `mariadb-<ver>-linux-systemd-x86_64/` containing `bin/`, `scripts/`, `lib/`, `share/`, `include/`, `man/`, `support-files/`, `mariadb-test/`, `sql-bench/`, docs/COPYING. Strip the top-level dir so `bin/` and `scripts/` are directly under the runtime root.

Key binaries in `bin/`: `mariadbd`, `mariadb`, `mariadb-admin`, `mariadb-dump`, `mariadb-check`, `mariadb-upgrade`, ... and MySQL compatibility names/symlinks: `mysqld -> mariadbd`, `mysql -> mariadb`, `mysqladmin -> mariadb-admin`, `mysqld_safe -> mariadbd-safe`, etc. `mariadb-install-db` is in `scripts/` (shell script), `mysql_install_db` is a symlink to it in `scripts/`.

## Host libraries / glibc floor

`readelf -V bin/mariadbd | grep GLIBC_` max: **GLIBC_2.28** (11.8.10).

`ldd bin/mariadbd`: resolves `libsystemd.so.0, libpthread, libdl, libstdc++, libm, libgcc_s, libc, ld-linux`; **`libcrypt.so.1 => not found`** on this Fedora 44 host (Fedora provides `libcrypt.so.2`, XCRYPT_2.0). `libcrypt.so.2` exports `crypt@XCRYPT_2.0` which satisfies the mariadbd undefined symbol; a local symlink/shim made the binary run. This is a host preflight concern for the packaged runtime (same class as Valkey's libsystemd/openssl deps) - the adapter must not assume the host has libcrypt.so.1.

`ldd bin/mariadb` (client): no libcrypt dependency.

## mariadb-install-db

Command used:

```sh
scripts/mariadb-install-db --datadir=<dir> --basedir=<runtime> \
  --auth-root-authentication-method=normal --skip-test-db
```

Observed: exit 0, ~0.66s on 11.8.10, creates system tables, prints "OK". It does not create a config file. With `--auth-root-authentication-method=normal` the root accounts have an empty plugin/authentication_string/password (passwordless native auth), verifiable by starting the server and querying `mysql.user`: root@localhost/127.0.0.1/::1/<hostname> have `plugin=''`, `authentication_string=''`, `Password=''`. `mysql.sys@localhost` has `mysql_native_password`.

The task requires setting superuser password (or keeping none for `AuthMode::None`) without putting it in argv, a file other than 0600, or a log. Options: `--init-file` (0600, deleted at once) with `ALTER USER 'root'@'localhost' IDENTIFIED BY ...`; or a protocol connection after a temporary start. The init-file approach is simple and matches MySQL practice; it needs the server started once. The adapter should prefer an init-file passed to a one-shot bootstrap start or use `mariadb-install-db` then start with `--init-file` - to be decided in implementation with a test proving no password leak.

## Server start / listeners

Started as `bin/mariadbd --defaults-file=<cnf> --user=<user>` with config:

```ini
[mysqld]
basedir=...
datadir=...
bind-address=127.0.0.1
port=43118
socket=.../mariadb.sock
pid-file=.../mariadbd.pid
log-error=.../mariadbd.err
skip-name-resolve
```

`ss -ltnp` showed `LISTEN 127.0.0.1:43118 users:(("mariadbd",pid=...))` only - no 0.0.0.0, no extra TCP listeners. A unix socket was also created at the configured path. The pidfile was written.

## Root auth / connection

With `--auth-root-authentication-method=normal`, `mariadb -u root -S <sock>` connected without password. A TCP login with empty password returned OK. A wrong password returned `ERR 1045 Access denied for user 'root'@'127.0.0.1' (using password: YES)`.

## Greeting / handshake

Captured greeting (port 43118, MariaDB 11.8.10): protocol 10, version `11.8.10-MariaDB`, charset 45, status 2, caps `0x81fffffe`, auth_plugin `mysql_native_password`. Raw greeting payload hex is recorded in tests as a fixture. An authentication challenge (ERR 1045) proves the server is up - same readiness rule as PostgreSQL/Valkey.

## Shutdown

`mariadb-admin -u root -S <sock> shutdown`: clean shutdown, log ends with `Shutdown completed; log sequence number ...` and `/.../mariadbd: Shutdown complete`. Exit 0.

`SIGTERM` to mariadbd: also clean shutdown, log shows `(initiated by: unknown): Normal shutdown` and ends with `Shutdown complete`. Acceptable as graceful stop if bounded and verified.

## Failure texts (verbatim, for diagnosis)

### Port in use (port 43119 already bound)
```
2026-11-22  8:03:14 0 [Note] Server socket created on IP: '127.0.0.1', port: '43119'.
2026-11-22  8:03:14 0 [ERROR] Can't start server: Bind on TCP/IP port. Got error: 98: Address already in use
2026-11-22  8:03:14 0 [ERROR] Do you already have another server running on port: 43119 ?
2026-11-22  8:03:14 0 [ERROR] Aborting
```

### Bad / missing data directory
```
2026-11-22  8:03:26 0 [Warning] Can't create test file '/tmp/w3b2/baddata/data/rezkycodes.lower-test' (Errcode: 2 "No such file or directory")
/tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: Can't change dir to '/tmp/w3b2/baddata/data/' (Errcode: 2 "No such file or directory")
2026-11-22  8:03:26 0 [ERROR] Aborting
```

### Bad config / unknown variable
```
2026-11-22  8:03:34 0 [ERROR] /tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: unknown variable 'bogus_option_that_does_not_exist=1'
2026-11-22  8:03:34 0 [ERROR] Aborting
```

### Permission denied on data dir
```
2026-11-22  8:03:50 0 [Warning] Can't create test file '/tmp/w3b2/perm/data/rezkycodes.lower-test' (Errcode: 13 "Permission denied")
/tmp/w3b2/spike/mariadb-11.8.10-linux-systemd-x86_64/bin/mariadbd: Can't change dir to '/tmp/w3b2/perm/data/' (Errcode: 13 "Permission denied")
2026-11-22  8:03:50 0 [ERROR] Aborting
```

### Socket path too long (> 107)
```
2026-11-22  8:04:00 0 [Note] Server socket created on IP: '127.0.0.1', port: '43123'.
2026-11-22  8:04:00 0 [ERROR] The socket file path is too long (> 107): /tmp/w3b2/socklong/rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr/m.sock
2026-11-22  8:04:00 0 [ERROR] Aborting
```

### MySQL 8.4 data dir opened by MariaDB 11.8.10
```
2026-11-22  8:11:47 0 [ERROR] InnoDB: File ./ib_logfile0 was not found
2026-11-22  8:11:47 0 [ERROR] InnoDB: Plugin initialization aborted with error Generic error
2026-11-22  8:11:47 0 [ERROR] Plugin 'InnoDB' registration as a STORAGE ENGINE failed.
2026-11-22  8:11:47 0 [ERROR] Unknown/unsupported storage engine: InnoDB
2026-11-22  8:11:47 0 [ERROR] Aborting
```

### MariaDB 11.4 datadir opened by 11.8 / 11.8 by 11.4

In this spike, opening an 11.4.14 datadir with 11.8.10 and an 11.8.10 datadir with 11.4.14 both reached "ready for connections" (InnoDB tolerated the format). That means "another major" is not always rejected; the adapter diagnosis should match explicit version-mismatch markers when present, but must not claim a mismatch it did not observe. A real incompatible case (MySQL 8.4 datadir) produces InnoDB errors as above.

## Socket path limit

AF_UNIX sun_path is 107 bytes on Linux. MariaDB refuses paths longer than 107 with the explicit error above. The adapter must check at create/start time and fail with a clear typed error before spawning.

## Notes for adapter

- Use `mariadbd` binary (or `mariadbd.exe` on Windows). Compat names exist but canonical name is clearer.
- `mariadb-install-db` lives in `scripts/`, not `bin/`.
- Config must force `bind-address=127.0.0.1`, `skip-name-resolve`, explicit `port`, `datadir`, short `socket`, `pid-file`, `log-error`.
- Password bootstrap must not leak into argv/env/log/world-readable file. Init-file (0600, deleted) or protocol bootstrap is required.
- Health check: TCP connect + read greeting packet; protocol version 10 means server is up. Auth challenge on login also means up.
- Shutdown: SIGTERM is graceful (log shows "Shutdown complete"); `mariadb-admin shutdown` also works. Adapter should prefer a bounded graceful shutdown; SIGTERM is acceptable given the spike.
- URI: `mysql://user:pw@127.0.0.1:port/` (no database name required; MariaDB/MySQL clients accept it).
- libcrypt.so.1 host dependency: report in preflight/diagnosis if binary fails to exec with loader error.
