//! User-facing messages for client errors. No secrets.

pub const UNSUPPORTED_CLIENT: &str = "This client does not support the selected engine.";
pub const CLIENT_NOT_FOUND: &str =
    "Client not found. It may have been deleted or moved.";
pub const LAUNCH_FAILED: &str = "Could not open the client. Check the client installation and try again.";
pub const INVALID_TARGET: &str = "The connection details are not valid.";
pub const NO_TERMINAL: &str = "No terminal found. Install a terminal emulator and try again.";
pub const SCRIPT_ERROR: &str = "Could not prepare the secure launcher script.";
