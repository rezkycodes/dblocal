//! Prints the measured contrast table from the real theme files, and fails if
//! any rendered pair is below WCAG AA. `cargo test -p dblocal-gui --test
//! contrast_table -- --nocapture` shows the numbers the final report quotes.

#[test]
fn theme_contrast_table_is_aa_and_printable() {
    match dblocal_gui::verify_theme_contrast() {
        Ok(table) => {
            println!("\n{table}");
            assert!(table.contains("DBLocal One Dark"));
            assert!(table.contains("DBLocal One Light"));
        }
        Err(failures) => panic!("contrast failures:\n{failures}"),
    }
}
