use dblocal_core::Result;

/// Launches the DBLocal GUI.
///
/// The real implementation is provided by a later worker once the GPUI
/// dependency is added.
pub fn run() -> Result<()> {
    Err(dblocal_core::DblError::NotImplemented(
        "GUI is not implemented yet".to_owned(),
    ))
}
