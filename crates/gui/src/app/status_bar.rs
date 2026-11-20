//! Status bar: running count, catalog freshness, downloads popover, tray and
//! version (`UI-DIRECTION.md`).

use dblocal_core::model::CatalogStatus;
use dblocal_core::strings;
use gpui::{InteractiveElement as _, IntoElement, ParentElement, Styled};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    progress::Progress,
};

use crate::format;

use super::Workspace;

/// Renders the bottom status bar.
pub(super) fn render(workspace: &Workspace, cx: &mut gpui::Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme();
    let running = workspace.running_count_value();
    let total = workspace.instances().len();
    let count = format!(
        "{running} {} / {total} {}",
        strings::UI_RUNNING,
        strings::UI_TOTAL
    );

    let catalog_text = match workspace.catalog().status {
        CatalogStatus::Fresh => format!(
            "{}: {}",
            strings::UI_CATALOG,
            format::catalog_status_label(workspace.catalog())
        ),
        CatalogStatus::Stale => {
            format!("{}: {}", strings::UI_CATALOG, strings::CATALOG_STALE)
        }
        CatalogStatus::Offline => {
            format!("{}: {}", strings::UI_CATALOG, strings::CATALOG_OFFLINE)
        }
        _ => format!(
            "{}: {}",
            strings::UI_CATALOG,
            format::catalog_status_label(workspace.catalog())
        ),
    };
    let catalog_colour = match workspace.catalog().status {
        CatalogStatus::Fresh => theme.muted_foreground,
        CatalogStatus::Stale => theme.warning,
        CatalogStatus::Offline => theme.danger,
        _ => theme.muted_foreground,
    };

    let downloads = workspace.downloads();
    let download_summary = if downloads.is_empty() {
        strings::UI_NO_DOWNLOADS.to_owned()
    } else {
        let percent = downloads
            .iter()
            .map(|download| {
                format::download_row(
                    dblocal_core::model::EngineKey::Postgresql,
                    "",
                    download.progress.bytes_done,
                    download.progress.bytes_total,
                    download.progress.bytes_per_sec,
                    download.progress.eta.map(|d| d.as_secs()),
                )
            })
            .map(|row| row.percent)
            .max()
            .unwrap_or(0);
        format!("{} ({percent}%)", strings::UI_DOWNLOADS)
    };

    let download_rows_inner: Vec<(String, u8, String, Option<String>)> = downloads
        .iter()
        .map(|download| {
            let row = format::download_row(
                dblocal_core::model::EngineKey::Postgresql,
                "",
                download.progress.bytes_done,
                download.progress.bytes_total,
                download.progress.bytes_per_sec,
                download.progress.eta.map(|d| d.as_secs()),
            );
            (download.title.clone(), row.percent, row.detail, row.eta)
        })
        .collect();

    let popover = Popover::new("downloads-pop")
        .anchor(gpui::Anchor::TopRight)
        .trigger(
            Button::new("downloads-trigger")
                .label(download_summary)
                .xsmall()
                .ghost()
                .compact(),
        )
        .content(move |_, _, cx| {
            let download_rows = download_rows_inner.clone();
            let theme = cx.theme();
            let mut column = gpui_component::v_flex().gap_2().p_3().min_w(gpui::px(280.));
            column = column.child(
                gpui::div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(strings::UI_DOWNLOADS),
            );
            if download_rows.is_empty() {
                column = column.child(
                    gpui::div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(strings::UI_NO_DOWNLOADS),
                );
            }
            for (title, percent, detail, eta) in download_rows {
                column = column.child(
                    gpui_component::v_flex()
                        .gap_1()
                        .child(gpui::div().text_xs().child(title))
                        .child(
                            Progress::new("download-row")
                                .value(percent as f32)
                                .color(theme.progress_bar)
                                .h(gpui::px(6.)),
                        )
                        .child(
                            gpui::div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(match eta {
                                    Some(eta) => format!("{detail}  ·  {eta}"),
                                    None => detail,
                                }),
                        ),
                );
            }
            column.into_any_element()
        });

    h_flex()
        .id("status-bar")
        .h(gpui::px(26.))
        .w_full()
        .items_center()
        .justify_between()
        .px_3()
        .bg(theme.status_bar)
        .border_t_1()
        .border_color(theme.status_bar_border)
        .text_xs()
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(gpui::div().child(count))
                .child(gpui::div().text_color(catalog_colour).child(catalog_text)),
        )
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(popover)
                .child(gpui::div().text_color(theme.muted_foreground).child(
                    if workspace.tray_available() {
                        strings::UI_TRAY_ACTIVE.to_owned()
                    } else {
                        strings::UI_TRAY_UNAVAILABLE.to_owned()
                    },
                ))
                .child(
                    gpui::div()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{}{}",
                            strings::UI_VERSION_PREFIX,
                            workspace.version()
                        )),
                ),
        )
}
