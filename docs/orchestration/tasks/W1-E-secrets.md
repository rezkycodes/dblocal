# W1-E — Credentials: OS keychain, file fallback, password generation

Branch: `w1-secrets`. Read first: AGENTS.md (invariants 4, 5), `docs/architecture/CONTRACT.md` (`SecretString`, `Credential`, `CredentialRef`), PRD §6 `F-CRD-01`,
`F-CRD-03`, decision D-17. Use Context7 (`resolve-library-id` then `query-docs`) for the current `keyring` crate documentation before choosing features.

## Goal

Store and retrieve an instance's password without ever putting it in SQLite, logs, errors, argv or `Debug` output.

## You own

`crates/core/src/secrets.rs`, `crates/core/src/secrets/**`, `docs/architecture/secrets.md`, and **only the lines you must add** for the `keyring` dependency in the root
`Cargo.toml` (`[workspace.dependencies]`) and `crates/core/Cargo.toml`. Keep that diff minimal and alphabetical. `keyring` is not in the skeleton yet.

## Deliverables

- `SecretBackend` trait (`get`, `set`, `delete` by `CredentialRef`, plus `kind() -> BackendKind`) returning `SecretString`.
- `KeyringBackend` using the `keyring` crate. **It must compile and `cargo xtask ci` must pass on this machine, which has no `libsecret-1` or `libdbus` development
  packages.** Investigate the crate's features: a pure-Rust Secret Service path (zbus) may avoid the C library; the kernel keyutils backend has no C dependency but does not
  survive a reboot, so it is not acceptable as the persistent store. Record what you chose, why, and what runs where in `docs/architecture/secrets.md`.
- `FileBackend`: `~/.dblocal/secrets/<ref>`, directory `0700`, files `0600` (Unix; best-effort ACL on Windows, documented), atomic write, refuses to follow symlinks, never
  world-readable. Chosen automatically when the keychain is unavailable, and the choice is reported through `kind()` so the UI can tell the user (F-CRD-01).
- `SecretStore` that tries the keychain first, falls back to the file, and exposes which backend each credential lives in. Migration between backends is out of scope.
- `generate_password()`: OS randomness (`rand`), length from `consts::PASSWORD_LEN`, unbiased alphabet selection (no modulo bias), characters chosen so the value is safe in
  a shell and a config file. A second helper builds the "no password" mode value (F-CRD-03) so nothing else has to special-case it.
- Zeroization on drop for everything holding a secret (`zeroize` is declared); `Debug`/`Display` redacted; error values never contain the secret.

## Tests (RED first)

Fake backend for logic; file backend creates `0700`/`0600` and the test asserts the modes; refuses a symlinked path; concurrent writers cannot interleave a torn file;
fallback is used when the keychain backend returns "unavailable"; password length, charset and a chi-square-style sanity check over many samples; a test that
formats every public type with `{:?}` and `{}` and asserts the secret never appears. A real-keychain smoke test exists but is `#[ignore]` and says why.

## Done when

`cargo xtask ci` green, >= 80% lines on files you own, report states exactly which keychain backends were compiled and which were run.
