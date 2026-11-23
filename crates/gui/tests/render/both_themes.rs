//! `F-PLH-02` AC1: every screen and overlay renders in **both** themes.
//!
//! Included from `tests/render.rs` (it uses that file's `render`, `Build` and
//! `Workspace`). Each case is drawn dark and light; both frames must be
//! non-trivial (many distinct colours, i.e. text and surfaces are present) and
//! the two themes must differ (a light frame is lighter). This is the "both
//! themes render every screen" check without committing a second screenshot per
//! case.

use super::{Build, Workspace, render};

#[test]
fn every_screen_renders_in_both_themes() {
    let cases: [(&str, Build); 9] = [
        ("overview", Workspace::preview),
        ("create", Workspace::preview_create),
        ("delete", Workspace::preview_delete),
        ("palette", Workspace::preview_palette),
        ("settings", Workspace::preview_settings),
        ("databases", Workspace::preview_product_databases),
        ("config", Workspace::preview_product_config),
        ("quit", Workspace::preview_quit),
        ("failure", Workspace::preview_failure),
    ];
    for (name, build) in cases {
        let dark = render(true, &format!("both-{name}-dark.png"), build);
        let light = render(false, &format!("both-{name}-light.png"), build);
        eprintln!(
            "BOTH {name}: dark_colours={} light_colours={}",
            dark.colours, light.colours
        );
        assert!(
            dark.colours > 24,
            "{name}: dark render too flat ({} colours)",
            dark.colours
        );
        assert!(
            light.colours > 24,
            "{name}: light render too flat ({} colours)",
            light.colours
        );
        assert!(
            light.mean_luminance() > dark.mean_luminance(),
            "{name}: the light frame is not lighter than the dark one"
        );
    }
}
