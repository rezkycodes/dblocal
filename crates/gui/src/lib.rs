use dblocal_core::Result;

/// Launches the DBLocal GUI.
///
/// Phase 0 stub: the real GPUI application lands with W1-G. Until then this
/// returns success so `dblocal` with no arguments behaves as the finished
/// binary does.
pub fn run() -> Result<()> {
    tracing::info!("dblocal-gui: no GUI yet (W1-G builds it)");
    Ok(())
}
