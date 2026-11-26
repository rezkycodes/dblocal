# Error code ranges

Every user-facing failure carries a stable `DBL-NNNN` code. `crates/core/src/error.rs` owns the codes of the core `DblError` enum:

| Range | Owner | Meaning |
|---|---|---|
| 1xxx | `error.rs` | invalid input, not found, already exists, invalid transition, port in use |
| 2xxx | `error.rs` | archive: checksum mismatch, unsafe archive |
| 3xxx | `error.rs` | I/O, network, storage |
| 4xxx | `error.rs` | engine |
| 5xxx | `error.rs` | secrets |
| 6xxx | `error.rs` | unsupported |
| 7xxx | `error.rs` | cancelled |
| 9xxx | `error.rs` | not implemented |

New feature modules do NOT edit `error.rs` or `strings.rs` (several workers edit around them at once). Instead each module keeps its own error type and its own range, and converts to the public `UserFacingError` (`UserFacingError::new(code, message)`, already public):

| Range | Module | Worker/task |
|---|---|---|
| 8100-8199 | `admin` (database and user administration) | W3-A |
| 8200-8299 | `update` (self-update) | W3-U |
| 8300-8399 | `clients` (open in an external client) | W3-C |
| 8400-8499 | `cli` (command line only: argument and terminal problems) | W3-D |
| 8500-8599 | packaging and autostart helpers (`autostart`, W4-D) | W4-D |
| 8600-8699 | `config` (config editor, F-CFG) | W4-C |
| 8700-8999 | reserved | |

Rules for a module error type:

1. A small enum with `fn code(&self) -> &'static str` (a `const` per variant, in your range) and `fn user_message(&self) -> &'static str` (English, no secrets, no SQL text, no paths that contain secrets). The messages live in a file inside the module (for example `admin/messages.rs`).
2. `impl From<YourError> for UserFacingError` using `UserFacingError::new(code, message)`.
3. `Debug` and `Display` never contain a password, a token, a connection string or the text of a failing statement; test it with a canary secret across every variant.
4. A table test asserts that every variant's code is inside your range and that codes are unique.
5. Integrating the module into `DblError` (a new variant, the `code()` mapping, strings) is the job of the task that wires the module into `DblService`, not of the module's author.
