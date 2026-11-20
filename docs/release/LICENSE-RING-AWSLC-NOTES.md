# Licence note: `ring` / `aws-lc-rs` / `rustls` and GPL-3.0

**Date:** 2026-11-20 (W0-D research)  
**Applies to:** DBLocal (GPL-3.0-only) and its future use of `reqwest` + `rustls`  
**Status:** researched, but **unverified against a resolved Cargo.lock** (no workspace exists yet in Wave 0).

## The concern

DBLocal is GPL-3.0-only and requires rustls-only TLS (no `openssl`, no `native-tls`). The
default crypto providers for `rustls` 0.23 are `ring` and/or `aws-lc-rs` (via `aws-lc-sys`). Both
codebases originate from **BoringSSL**, which was forked from **OpenSSL before OpenSSL 3.0's
relicensing to Apache-2.0**. The pre-3.0 OpenSSL licence is a conjunction of the "OpenSSL License"
and the "SSLeay License" (an advertising-clause BSD-style licence). The FSF states this
combination is **"a copyleft free software license that is incompatible with the GNU GPL"** and
also notes its advertising clause (like the original BSD / Apache 1.x licences). That is why
`ring`'s historical licence (SPDX including `AND OpenSSL`) was a real blocker for GPL projects.

## What the published crates say today (checked 2026-11-20, crates.io static downloads)

| Crate | Version checked | SPDX (Cargo.toml / crates.io) | Old "OpenSSL License" text present in crate? |
|---|---:|---|---|
| `ring` | 0.17.14 | `Apache-2.0 AND ISC` | **No.** `LICENSE` says "BoringSSL relicensed to Apache 2.0"; `LICENSE-BoringSSL` is Apache-2.0; no `OpenSSL License` header in any `.c/.h/.rs/.S` file in the tarball. |
| `aws-lc-sys` | 0.47.0 | `ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)` | **No.** Top-level `LICENSE` documents BoringSSL relicensed to Apache-2.0 and OpenSSL-derived code as `SPDX-License-Identifier: Apache-2.0`. |
| `aws-lc-rs` | 1.19.0 | `ISC AND (Apache-2.0 OR ISC)` (per crate metadata) | — (wraps `aws-lc-sys`) |
| `rustls` | 0.23.46 | `Apache-2.0 OR ISC OR MIT` | — (crypto provider is feature-selected) |

This matches upstream history:

- **ring#1827** (*"Stop using OpenSSL-license-licensed code"*, opened 2023-11-27, **closed completed
  2025-02-19**, briansmith/ring): the OpenSSL-licensed C code was replaced and/or upstream BoringSSL
  relicensed to Apache-2.0; the crate's published licence became `Apache-2.0 AND ISC`.
- **rustls#1893** (*"Incompatible License"*, closed 2024-04-10, not planned): rustls maintainers
  noted the BoringSSL/OpenSSL-derivation has "existed forever" in the dependency chain and pointed
  GPL-constrained users at opting out of ring/aws-lc-rs and supplying a `CryptoProvider` from
  GPL-compatible dependencies instead. That answer predates ring's 2025 relicensing cleanup.

## Is this a real GPL-3.0 problem today?

**For the specific published versions above, the SPDX metadata and shipped licence texts do not
contain the old OpenSSL licence.** Apache-2.0 and ISC are both GPL-3.0-compatible (FSF). Taken at
face value, `ring 0.17.14` and `aws-lc-sys 0.47.0` do **not** trigger the old OpenSSL/GPL
incompatibility.

**But it is not a fully settled non-issue for this project yet**, for three reasons:

1. **The dependency graph is not fixed.** W0-A has not produced a `Cargo.lock`. Until it does,
   `cargo deny check licenses` cannot run and we do not know which exact provider/crate versions the
   workspace will resolve, or whether any transitive dependency introduces an incompatible licence.
2. **Historical/derivation nuance.** Even with relicensing, code originally copied under the
   pre-3.0 OpenSSL/SSLeay terms was once under a GPL-incompatible licence. Most downstream
   redistributors treat the upstream relicensing + current SPDX as authoritative for the shipped
   work, but a cautious GPL compliance posture should acknowledge the provenance rather than assert
   certainty it cannot prove.
3. **Provider choice is a policy decision.** `rustls` 0.23 selects a crypto provider via features
   (e.g. `ring` or `aws-lc-rs`); the "right" default for a GPL-3.0 binary is worth choosing
   deliberately and pinning, not inheriting accidentally.

## What other GPL projects do

- **Use `rustls` with the `ring` provider (post-0.17 relicensing):** many GPL-3.0 Rust projects
  treat `ring 0.17.x` (`Apache-2.0 AND ISC`) as acceptable and rely on `cargo deny` to enforce the
  allowlist. This is the pragmatic, low-friction path when the resolved tree is clean.
- **Use `rustls` with `aws-lc-rs`:** acceptable on the same licence-compatibility grounds per its
  current SPDX, but pulls a larger/native build dependency (`aws-lc-sys` builds/ships C/asm via
  CMake/`cc`), which is operationally heavier.
- **Supply a `rustls::crypto::CryptoProvider` from elsewhere:** the `rustls` API is
  provider-pluggable; a GPL-strict project can opt out of both ring/aws-lc-rs and wire a provider
  backed only by dependencies it is comfortable with (for example a provider built on
  `*ring*`-compatible APIs is not an option, but projects have explored `RustCrypto`-based
  providers such as `rustls-rustcrypto` / community `rustls-graviola`-style efforts). Availability,
  maturity, FIPS posture and maintenance vary — none are as battle-tested as the defaults as of
  this writing, so this is a real trade-off, not a free win.
- **Keep `deny.toml` strict and treat exceptions as reportable decisions:** never add `OpenSSL` to
  the allowlist; if a future resolution reintroduces it, that is a release blocker until resolved.

## Recommendation

1. **Short term (Phase 1, when W0-A lands):** depend on `rustls` with an **explicitly chosen crypto
   provider** — prefer **`ring`** (`rustls` feature selecting ring, default-features adjusted to
   avoid pulling aws-lc-rs) and pin/record the chosen feature set. `ring 0.17.14` is
   `Apache-2.0 AND ISC` with no OpenSSL licence text in the published crate, which is the cleanest
   available default.
2. **Enforce mechanically:** keep `deny.toml` as written (`OpenSSL` not allowed; bans on
   `openssl`/`openssl-sys`/`native-tls`; crates.io-only sources). Run `cargo deny check licenses
   bans sources advisories` in CI and locally, and **inspect the resolved licence report** — do not
   just trust the allowlist in the abstract.
3. **Document the decision:** when the workspace is created, record the selected provider and the
   `cargo deny` result in the release/architecture notes so the provenance question above is visible
   to future contributors, not hidden.
4. **Contingency:** if a resolved dependency ever brings in the old `OpenSSL` licence (or any
   non-allowlisted licence), **block the release** and either (a) pin away from the offending
   version, (b) switch providers, or (c) adopt an alternative `CryptoProvider`. Do not grant an
   `OpenSSL` exception for a GPL-3.0-only binary.
5. **Do not** "solve" this by adding a GPL+OpenSSL exception to DBLocal's own licence without legal
   review — that is a licensing change to the project, not a dependency configuration.

## Bottom line

The historical `ring`/BoringSSL-vs-GPL problem is **largely resolved upstream** in the published
crates (Apache-2.0/ISC, no OpenSSL licence text). For a GPL-3.0-only binary using
`reqwest`+`rustls`, the practical, defensible choice today is **`rustls` with the `ring` provider,
explicitly pinned, and `cargo deny` gating every release** — while treating the final answer as
confirmed only once a real `Cargo.lock` passes a clean `cargo deny check licenses`.

## Sources

- [briansmith/ring#1827 — Stop using OpenSSL-license-licensed code](https://github.com/briansmith/ring/issues/1827) (opened 2023-11-27, closed completed 2025-02-19)
- [rustls/rustls#1893 — Incompatible License](https://github.com/rustls/rustls/issues/1893) (closed 2024-04-10)
- [FSF: Various Licenses and Comments about Them — OpenSSL license](https://www.gnu.org/licenses/license-list.html#OpenSSL) (incompatible with GPL; old OpenSSL/SSLeay licence)
- [FSF: Various Licenses and Comments about Them — Apache License 2.0](https://www.gnu.org/licenses/license-list.html#apache2) (compatible with GPLv3)
- [FSF: Various Licenses and Comments about Them — ISC License](https://www.gnu.org/licenses/license-list.html#ISC) (compatible with GPL)
- [crates.io: ring 0.17.14](https://crates.io/crates/ring/0.17.14) — SPDX `Apache-2.0 AND ISC`, crate contents inspected
- [crates.io: aws-lc-sys 0.47.0](https://crates.io/crates/aws-lc-sys/0.47.0) — SPDX/LICENCE inspected
- [crates.io: rustls 0.23.46](https://crates.io/crates/rustls/0.23.46) — SPDX `Apache-2.0 OR ISC OR MIT`
- [Rustls: cryptography providers / CryptoProvider](https://docs.rs/rustls/latest/rustls/crypto/struct.CryptoProvider.html)
- [EmbarkStudios/cargo-deny](https://embarkstudios.github.io/cargo-deny/) (licence/advisory/bans/sources checks)
