# Credential storage (W1-E)

This document records how DBLocal stores instance passwords, which OS keychain
backends it compiles and which ones were actually exercised on the build
machine. It is the reference for PRD `F-CRD-01`, `F-CRD-03` and decision D-17.

Code: `crates/core/src/secrets.rs` and `crates/core/src/secrets/`.
Hard invariants: AGENTS.md §3 items 4 and 5.

## What is stored, and what is not

| Item | Where it lives |
|---|---|
| The password itself | OS keychain, or `~/.dblocal/secrets/<ref>` |
| `CredentialBackend` (which one) | `state.db`, `instances.credential_backend` |
| Key reference (`dblocal:<instance-id>`) | `state.db`, computed by `credential_ref_for` |
| Username | `state.db` (**not** a secret) |
| The password in a log/error/argv/`Debug` | never |

`CredentialRef` is a pointer: instance id, backend, key reference, username. It
holds no secret. The only way to read the password is
`SecretStore::get(_)`/`get_from(_, _)`, which returns a `SecretString` that
redacts `Debug`/`Display` and zeroes its buffer on drop.

## Backends

```text
                 ┌────────────────────────────┐
   set/get/del   │ SecretStore                │
   ─────────────►│  keyring first             │
                 │  on Unsupported → file     │
                 └───────┬────────────┬───────┘
                         │            │
                 ┌───────▼──────┐ ┌───▼─────────┐
                 │ KeyringBackend│ │ FileBackend │
                 │ (keyring 4.2) │ │ 0600 files  │
                 └───────────────┘ └─────────────┘
```

`SecretBackend` is the trait: `kind() -> BackendKind`, `get`, `set`, `delete`,
each keyed by `CredentialRef` and returning/taking `SecretString`.

`BackendKind` is a type alias of `CredentialBackend`, not a second enum. The
persisted column, the read model (`InstanceView.credential_backend`) and the
trait therefore cannot drift apart.

### `KeyringBackend` — OS keychain

Uses `keyring = "4.2"` with its **default `v1` feature**. That feature selects,
per target:

| Platform | Store crate selected by `v1` | Language / system dependency |
|---|---|---|
| Linux / \*nix (not macOS/iOS/Android) | `zbus-secret-service-keyring-store` 1.x | **Pure Rust.** D-Bus over `zbus`; Secret Service crypto via `secret-service/crypto-rust` |
| macOS / iOS | `apple-native-keyring-store` (keychain) | Apple Security framework |
| Windows | `windows-native-keyring-store` | Windows Credential Manager |

**Why not libsecret or a dbus C library:** the build machine has no
`libsecret-1` or `libdbus` development packages, and AGENTS.md forbids installing
system libraries or silently vendoring a C library. The `zbus` Secret Service
store speaks the same FreeDesktop Secret Service API as libsecret but implements
the wire protocol in Rust, so it builds and links with no `pkg-config` dependency.
`openssl` is not pulled in: `crypto-rust` selects `aes`/`cbc`/`hkdf`/`sha2`
instead of `crypto-openssl`, which also keeps `cargo deny check bans`
(openssl/native-tls forbidden) green.

Linux keyring keys are the `service` = `dblocal` and `user` = the reference's
`key_ref` (`dblocal:<instance-id>`), matching `F-CRD-01` AC1.

**Rejected alternatives**

- `linux-keyutils-keyring-store` (kernel keyring): no C dependency, but
  credentials do not survive a reboot (`CredentialPersistence::UntilReboot`).
  That fails the "persistent store" requirement, so it is not used.
- `dbus-secret-service-keyring-store`: depends on the `dbus` C library / system
  D-Bus development files. Not available on this machine.

**Error classification.** `keyring` errors are mapped in one place:

| `keyring` error | DBLocal error | Meaning |
|---|---|---|
| `NoEntry` | `DblError::NotFound` (DBL-1002) | Nothing stored; normal |
| `NoDefaultStore`, `NoStorageAccess`, `PlatformFailure` | `DblError::Unsupported` (DBL-6001) | Keychain unusable **now** → fall back |
| `BadEncoding`, `BadStoreFormat`, `TooLong`, `Invalid`, `Ambiguous`, `NotSupportedByStore` | `DblError::Secret` (DBL-5001) | Store rejected the value; do not fall back |
| unknown (`#[non_exhaustive]`) | `DblError::Unsupported` | Fall back rather than hard-fail |

Only `Unsupported` triggers the file fallback. A `NotFound` from a healthy
keychain is a real answer and does **not** cause a file read, so a stale file
can never resurrect a deleted credential. Upstream error text is never copied
into a `DblError`, because a store implementation may embed the secret in its
message; only a debug-level log line records the mapping.

### `FileBackend` — `0600` fallback

Path: `<root>/secrets/<key_ref>`, i.e. `~/.dblocal/secrets/dblocal:<instance-id>`
in production and `<DBLOCAL_HOME>/secrets/...` in tests.

| Property | Unix | Windows |
|---|---|---|
| Directory mode | `0700`, set explicitly after create; recursive parents | inherits parent ACL |
| File mode | `0600`, forced after create (umask cannot widen it) | inherits parent ACL (best effort) |
| Symlinked directory | refused (`symlink_metadata`) | refused |
| Symlinked file | refused, checked before every get/set/delete **and** via `O_NOFOLLOW` | refused (check only) |
| Atomic write | temp file in same dir, `fsync`, `rename`, best-effort dir `fsync` | same |
| Never world-readable | guaranteed on Unix; on Windows only as good as the user profile ACL | documented limitation |

The temp file is created with `create_new(true)` and mode `0600` **before** any
byte is written, so the plaintext is never briefly world-readable. Two writers
in one process never collide because the temp name carries a process-wide
atomic counter and the PID; the rename is atomic, so a reader sees the old value
or the new one, never a torn file (covered by a two-writer test).

Windows: `std::fs` cannot apply a POSIX mode. The file inherits the ACL of
`%USERPROFILE%\.dblocal\secrets`, which is normally user-only. A full ACL
restriction would need `windows-sys` ACL APIs; that is documented here as
best-effort rather than implemented.

### `SecretStore` — keychain first, file fallback

- `set` tries the keychain; on `Unsupported` it writes the file and returns the
  backend that actually stored the value. That return value is what must be
  persisted so `F-CRD-01` AC2 ("Disimpan di file, bukan keychain") is honest.
- `get` tries the keychain; on `Unsupported` it reads the file.
- `delete` follows the same rule.
- `delete_everywhere` removes from both and tolerates `NotFound`, which is what
  `F-CRD-01` AC3 (delete on instance removal) needs even if the keychain went
  away in the meantime.
- `get_from(kind, ref)` reads the backend recorded on the instance and falls
  back to the other one only when the recorded backend is `Unsupported`, so a
  credential recorded as keychain-backed stays readable on a headless login.

Migration between backends is **out of scope** (task statement); nothing here
moves a secret from one backend to the other.

## Password generation (`F-CRD-01`, `F-CRD-03`)

`generate_password()` returns `consts::PASSWORD_LEN` (= 24) characters drawn
from `ALPHABET`:

```text
ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789
```

- Source: `rand::rngs::SysRng`, a stateless interface over the OS CSPRNG
  (`getrandom`). Entropy failure becomes `DblError::Secret`; it never panics.
- **No modulo bias:** bytes are drawn one `u32` at a time, masked down to 6 bits
  (`mask = next_power_of_two(62) - 1`), and values `>= 62` are rejected and
  redrawn. Every symbol is therefore exactly equally likely. (rand 0.10's
  `random_range` is documented as possibly biased unless the `unbiased` feature
  is enabled; rejection sampling makes the guarantee explicit and testable.)
- **Shell/config safe:** the alphabet excludes whitespace, quotes, `$`, `\`,
  `%`, `&`, `#`, `?`, `!`, `:`, `@`, `/`, `=`, `+`, `<`, `>`, `|`, `;`, `` ` ``
  and `,`. A generated value needs no escaping in a POSIX shell, a `.env` block,
  an engine config file or a connection URI. The percent-encoding requirement of
  `F-CRD-02` AC4 still exists for a user-supplied password, but never triggers
  for a generated one.

`InstanceSecret` is the second helper. `InstanceSecret::none()` is the value for
`F-CRD-03`, so no other module special-cases `AuthMode::None`.
`InstanceSecret::for_mode(auth, secret)` rejects an inconsistent pair (password
mode without a password, or no-auth mode carrying one).

## Tests

| Test | What it proves |
|---|---|
| `secrets::password::*` | length 24, allowed charset, no forbidden characters, zero length rejected, chi-square-style uniformity over 186 000 draws, CSPRNG failure mapping |
| `secrets::file::*` | `0700`/`0600` modes asserted, symlinked directory and file refused, concurrent writers never tear, unsafe keys rejected, non-UTF-8 rejected, delete semantics |
| `secrets::store::*` | keychain preferred; file used when keychain reports `Unsupported`; `NotFound` is not a fallback trigger; `delete_everywhere`; `get_from` recovery; torn-write test through the store |
| `secrets::keyring::*` | every `keyring` error variant mapped to the right `DblError` code, and get/set/delete driven through a scripted `KeychainOpener`; the platform opener never runs under `cargo test` |
| `secrets::redaction_tests::*` | every public type formatted with `{:?}` and `{}`, every error path, never contains the canary |
| `real_keychain_round_trip` | `#[ignore]`d; needs a live keychain and may prompt for unlock |

Run the ignored smoke test with:

```sh
export CARGO_TARGET_DIR="$HOME/.cache/dblocal-target"
cargo test -p dblocal-core --lib secrets::keyring -- --ignored
```

## What was compiled and what was run on this machine

Measured on the W1-E build host (Linux, no `libsecret-1`, no `libdbus` dev
packages):

**Compiled** (`cargo build -p dblocal-core`, target `x86_64-unknown-linux-gnu`):

- `keyring` 4.2.0 with `v1`
- `zbus-secret-service-keyring-store` 1.0.1
- `secret-service` 5.2.0 (`crypto-rust`, `zbus` async-io runtime)
- `zbus` 5.19.0
- **not** compiled: `apple-native-keyring-store`, `windows-native-keyring-store`,
  `android-native-keyring-store`, `dbus-secret-service-keyring-store`,
  `linux-keyutils-keyring-store`, `db-keystore`, `openssl`

**Run**:

- `cargo test` never touches a real keychain: `KeyringBackend` is driven through
  a scripted `KeychainOpener` in every test. This keeps the suite offline
  (AGENTS.md §4) and avoids writing to the developer's real keychain.
- The pure-Rust D-Bus Secret Service path was exercised manually on this host
  while choosing the feature set: a scratch crate reported
  `store_status() == Ok` and `set_password`/`get_password`/`delete_credential`
  all succeeded against the running session Secret Service. Those probe
  credentials were deleted afterwards.
- When no session bus is reachable, `store_status()` is `Err`, so
  `KeyringBackend::is_available()` is `false`, keychain operations answer
  `Unsupported`, and `SecretStore` writes and reads the `0600` file instead.
  Verified with `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent-bus`
  (`store err: Platform failure: zbus error: Failed to connect to address ...`).
- `real_keychain_round_trip` exists but is `#[ignore]`d; it is the only test
  that depends on a live keychain and may block on an unlock prompt.

## Invariants this module must keep

1. No password in SQLite, logs, errors, argv or `Debug`/`Display`.
2. Every secret-holding buffer is zeroed on drop (`SecretString` =
   `Zeroizing<String>`; `generate_with` zeroes its partial buffer if entropy
   fails).
3. The file fallback is never world-readable and never follows a symlink.
4. A credential lives in exactly one backend; the backend is recorded and
   reported, never guessed.
5. No elevation, no writes outside `~/.dblocal`.
