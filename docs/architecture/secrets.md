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
holds no secret. The only way to read the password is `SecretStore::get(&ref)`,
which routes by `ref.backend()` and returns a `SecretString` that redacts
`Debug`/`Display` and zeroes its buffer on drop.

## Backends

```text
                 ┌────────────────────────────┐
   set            │ SecretStore                │
   ──────────────►│  keychain first,           │
                  │  file on Unsupported       │
   get            │                            │
   ──────────────►│  routes by                 │
                  │  reference.backend() only  │
   delete         │                            │
   ──────────────►│  removes from both         │
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

Path: `<root>/secrets/<file-name>`, where `<file-name>` is derived from the key
reference by `file_name_for`. Every allowed character (`A-Za-z0-9`, `.`, `-`,
`_`) is kept, every `:` becomes `%3A`, and the whole name is prefixed `cred-`.
So `dblocal:<instance-id>` lives at `~/.dblocal/secrets/cred-dblocal%3A<id>`
(production) or `<DBLOCAL_HOME>/secrets/cred-dblocal%3A<id>` (tests).

Why encode at all: `:` is legal in the key (`dblocal:<instance-id>`) but on
NTFS `secrets\cred-dblocal:<id>` would name an *alternate data stream* of a file
called `cred-dblocal`, and renaming the temp file onto it fails, so the fallback
would not work on Windows at all. `%` is not a legal key character, so `%3A` can
only ever come from a `:`, which makes the mapping injective: two distinct keys
can never share a file name. The `cred-` prefix means a key that happens to be a
reserved Windows device name (`CON`, `NUL`, `COM1`, ...) is still an ordinary
file. `get`, `set` and `delete` all resolve the path through the same function,
and temp files keep their `.tmp-` prefix.

| Property | Unix | Windows |
|---|---|---|
| Directory mode | `0700`; created explicitly **and** an existing real directory is tightened to `0700` | inherits parent ACL |
| File mode | `0600`, forced after create (umask cannot widen it) | inherits parent ACL (best effort) |
| Symlinked directory | refused (`symlink_metadata`) | refused |
| Symlinked file | refused, checked before every get/set/delete **and** via `O_NOFOLLOW` | refused (check only) |
| File name | `cred-` + key with `:` → `%3A` (no `:`, never a device name) | same |
| Atomic write | temp file in same dir, `fsync`, `rename`, best-effort dir `fsync` | same |
| Never world-readable | guaranteed on Unix; on Windows only as good as the user profile ACL | documented limitation |

The directory tightening matters because an older build (or a hand-made
directory) could have created `secrets/` under the default umask (`0755`), which
lets other users list the reference names. `ensure_dir` therefore keeps refusing
a symlink but forces `0700` on any existing real directory before a write.

The temp file is created with `create_new(true)` and mode `0600` **before** any
byte is written, so the plaintext is never briefly world-readable. Two writers
in one process never collide because the temp name carries a process-wide
atomic counter and the PID; the rename is atomic, so a reader sees the old value
or the new one, never a torn file (covered by a two-writer test).

Windows: `std::fs` cannot apply a POSIX mode. The file inherits the ACL of
`%USERPROFILE%\.dblocal\secrets`, which is normally user-only. A full ACL
restriction would need `windows-sys` ACL APIs; that is documented here as
best-effort rather than implemented.

### `SecretStore` — routing

`set` prefers the keychain and falls back to the file when the keychain reports
itself unavailable, returning the backend that actually stored the value so the
caller persists it with the instance. `get` routes **strictly** by
`CredentialRef::backend()`, the backend recorded on the instance, and nothing
else: a credential lives in exactly one backend, so a read never crosses over.
This is what makes a secret survive the realistic "created over SSH, then opened
on the desktop" sequence — the secret was written to the file fallback while the
keychain was down, and a later `get` from a session where gnome-keyring *is* up
must not ask the (now available but empty) keychain and answer `NotFound`; it
reads the file, as recorded. Conversely, if the recorded keychain backend is
locked or gone, its `Unsupported` error is returned unchanged so the UI can say
the keychain is unavailable, and the file is never consulted for a secret it does
not hold. `delete` removes the credential from **both** backends, so a stale copy
can never survive an instance teardown; a missing entry or an unreachable backend
is not an error there, but an invalid key reference or a real I/O failure is.

Migration between backends is **out of scope** (task statement); nothing here
moves a secret from one backend to the other.

### Platform coverage (unverified)

This host is Linux, so only the `zbus` Secret Service path was built and
exercised. The `apple-native-keyring-store` (macOS Keychain) and
`windows-native-keyring-store` (Windows Credential Manager) stores are selected
by the same `v1` feature but sit behind `cfg` branches this machine cannot
compile or run; they are **unverified** here. Likewise the Windows filesystem
behaviour (ACL inheritance, the absence of `:` in names) is reasoned about and
encoded defensively but not run.

### Dependency footprint

Adding `keyring` 4.2 with the `v1` feature brings in **50 new transitive
crates** (essentially the `zbus` stack: `zbus`, `zvariant`, `secret-service`,
`async-io`, `futures-lite`, …). That is a real supply-chain increase, accepted
because it is the pure-Rust Secret Service path: it needs no `libsecret-1` or
`libdbus` development package, pulls in no `openssl`/`native-tls`, and adds no
`*-sys` crate and no `unsafe` in our code. `cargo deny check` passes
advisories, bans, licenses and sources with that graph.

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
| `secrets::file::*` | `0700`/`0600` modes asserted; an existing `0755` directory is tightened to `0700`; symlinked directory and file refused; concurrent writers never tear; unsafe keys rejected; non-UTF-8 rejected; delete semantics; the file name never contains `:`, is prefixed `cred-`, and 20 distinct keys (including `:` vs `_` pairs) give distinct names |
| `secrets::store::*` | `set` prefers the keychain and falls back to the file, returning the backend used; `get` routes by `reference.backend()` **only** — a file-recorded secret is readable when the keychain is available but empty, and an unavailable recorded keychain returns `Unsupported` without touching the file (asserted via call counters); `delete` clears both backends and is idempotent; torn-write test through the store |
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
