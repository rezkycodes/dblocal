//! Temporary probe: list duplicate painted-text paths per preview (not committed).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::collections::BTreeMap;
use std::sync::Arc;

use dblocal_core::service::{DblService, mock::MockService};
use dblocal_gui::app::Workspace;
use gpui_kit::TestAppContext;

type Build = fn(
    Arc<dyn DblService>,
    &mut gpui_kit::Window,
    &mut gpui_kit::Context<Workspace>,
) -> Workspace;

fn probe(cx: &mut TestAppContext, name: &str, build: Build) {
    cx.update(gpui_kit::init);
    cx.update(dblocal_gui::theme::init);
    let svc: Arc<dyn DblService> = Arc::new(MockService::demo());
    let (_view, cx) = cx.add_window_view(|window, cx| build(svc, window, cx));
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let (total, dups) = cx.update(|window, _| {
        let texts = gpui_kit::base::test_support::texts(window);
        let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
        for t in &texts {
            let key = format!("{:?}", t.path());
            let entry = seen.entry(key).or_insert((0, t.text().to_owned()));
            entry.0 += 1;
        }
        let mut out: Vec<(usize, String, String)> = seen
            .into_iter()
            .filter(|(_, (n, _))| *n > 1)
            .map(|(k, (n, txt))| (n, k, txt))
            .collect();
        out.sort();
        (texts.len(), out)
    });
    eprintln!("PROBE {name}: texts={total} dup-paths={}", dups.len());
    for (n, path, txt) in dups.iter().take(15) {
        eprintln!("  x{n} text={txt:?} path={path}");
    }
}

#[gpui_kit::test]
fn probe_all(cx: &mut TestAppContext) {
    probe(cx, "preview", Workspace::preview);
}

#[gpui_kit::test]
fn probe_quit(cx: &mut TestAppContext) {
    probe(cx, "quit", Workspace::preview_quit);
}

#[gpui_kit::test]
fn probe_create(cx: &mut TestAppContext) {
    probe(cx, "create", Workspace::preview_create);
}

#[gpui_kit::test]
fn probe_delete(cx: &mut TestAppContext) {
    probe(cx, "delete", Workspace::preview_delete);
}

#[gpui_kit::test]
fn probe_palette(cx: &mut TestAppContext) {
    probe(cx, "palette", Workspace::preview_palette);
}

#[gpui_kit::test]
fn probe_databases(cx: &mut TestAppContext) {
    probe(cx, "databases", Workspace::preview_databases);
}

#[gpui_kit::test]
fn probe_config(cx: &mut TestAppContext) {
    probe(cx, "config", Workspace::preview_product_config);
}

#[gpui_kit::test]
fn probe_logs(cx: &mut TestAppContext) {
    probe(cx, "logs", Workspace::preview_logs_long);
}

#[gpui_kit::test]
fn probe_failure(cx: &mut TestAppContext) {
    probe(cx, "failure", Workspace::preview_failure);
}
