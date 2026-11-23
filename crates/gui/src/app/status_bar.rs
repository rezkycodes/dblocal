//! The status bar: counts (running, stopped, failed), data directory and total
//! size, credential backend, `os arch`, app version, plus what W1-G already
//! showed — active downloads with their popover, the offline/stale banner and
//! the tray-unavailable note (D-25 prototype).

use dblocal_core::model::CatalogStatus;
use dblocal_core::strings;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme, Sizable, Theme, h_flex, popover::Popover};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use crate::format;

use super::Workspace;
use super::components::{icon, status_glyph};
use gpui_kit::assets::IconName;

/// Renders the bottom status bar.
pub(super) fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme().clone();
    let counts = workspace.counts();
    let total = workspace
        .total_home_size()
        .map(format::format_bytes_id)
        .unwrap_or_else(|| strings::UI_VALUE_UNKNOWN.to_owned());
    let data_dir = workspace.home_display();
    let backend = credential_backend_label(workspace);
    let platform = format::platform_label(std::env::consts::OS, std::env::consts::ARCH);

    h_flex()
        .id("status-bar")
        .h(px(28.))
        .w_full()
        .flex_none()
        .items_center()
        .gap_4()
        .px(px(14.))
        .bg(theme.status_bar)
        .border_t_1()
        .border_color(theme.status_bar_border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(count_pill(
            counts.running,
            strings::UI_RUNNING,
            &theme,
            false,
        ))
        .child(count_pill(
            counts.stopped,
            strings::UI_STOPPED,
            &theme,
            false,
        ))
        .when(counts.failed > 0, |this| {
            this.child(count_pill(counts.failed, strings::UI_FAILED, &theme, true))
        })
        .child(
            h_flex()
                .gap_1p5()
                .child(icon(IconName::HardDrive, 13.0, theme.muted_foreground))
                .child(
                    Label::new(format!("{data_dir} · {total}"))
                        .font_family(theme.mono_font_family.clone()),
                ),
        )
        .child(div().flex_1())
        .child(
            h_flex()
                .gap_1p5()
                .child(icon(IconName::KeyRound, 13.0, theme.muted_foreground))
                .child(Label::new(format!(
                    "{}: {backend}",
                    strings::UI_CREDENTIAL_BACKEND
                ))),
        )
        .child(Label::new(platform))
        .when(workspace.pending(), |this| {
            this.child(
                Label::new(strings::UI_PENDING)
                    .text_xs()
                    .px_2()
                    .rounded_full()
                    .bg(crate::theme::tokens::tint(theme.warning, 0.20))
                    .text_color(theme.warning),
            )
        })
        .child(Label::new(if workspace.tray_available() {
            strings::UI_TRAY_ACTIVE
        } else {
            strings::UI_TRAY_UNAVAILABLE
        }))
        .when_some(workspace.tray_note(), |this, note| {
            this.child(Label::new(note).text_xs().text_color(theme.warning))
        })
        .child(downloads_popover(workspace, &theme, cx))
        .when_some(workspace.toast_text(), |this, toast| {
            this.child(
                Label::new(toast)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .max_w(px(320.)),
            )
        })
        .child(Label::new(format!(
            "{}{}",
            strings::UI_VERSION_PREFIX,
            workspace.version()
        )))
}

/// One status-bar count: glyph plus the number and word.
///
/// A11y: `Label::new` takes its element id from the call site, so rendering
/// this helper three times (running, stopped, failed) made three siblings share
/// one id and gpui's duplicate-node assertion fired. The id is per count.
fn count_pill(count: usize, label: &str, theme: &Theme, danger: bool) -> impl IntoElement {
    let colour = if danger {
        theme.danger
    } else {
        theme.muted_foreground
    };
    let status = if danger {
        dblocal_core::model::InstanceStatus::Failed
    } else if label == strings::UI_RUNNING {
        dblocal_core::model::InstanceStatus::Running
    } else {
        dblocal_core::model::InstanceStatus::Stopped
    };
    h_flex()
        .gap_1p5()
        .text_color(colour)
        .child(status_glyph(status, 8.0, theme))
        .child(
            Label::new(format!("{count} {label}"))
                .id(SharedString::from(format!("status-count-{label}"))),
        )
}

/// Which credential backend the bar reports: the file fallback wins if any
/// instance uses it, so the note is never hidden (`F-CRD-01` AC2).
fn credential_backend_label(workspace: &Workspace) -> &'static str {
    let any_file = workspace
        .instances()
        .iter()
        .any(|view| format::credential_is_file(view.credential_backend));
    if any_file {
        strings::UI_CREDENTIAL_FILE
    } else {
        strings::UI_CREDENTIAL_KEYCHAIN
    }
}

/// The downloads trigger and its popover, unchanged in behaviour from W1-G.
fn downloads_popover(
    workspace: &Workspace,
    _theme: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let downloads = workspace.downloads();
    let summary = if downloads.is_empty() {
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
                .percent
            })
            .max()
            .unwrap_or(0);
        format!("{} ({percent}%)", strings::UI_DOWNLOADS)
    };
    let rows: Vec<(String, u8, String, Option<String>)> = downloads
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
    let _ = cx;
    let rows_for_popover = rows.clone();
    Popover::new("downloads-pop")
        .anchor(gpui_kit::Anchor::TopRight)
        .trigger(
            Button::new("downloads-trigger")
                .label(summary)
                .xsmall()
                .ghost()
                .compact(),
        )
        .content(move |_, _, cx| {
            let rows = rows_for_popover.clone();
            let theme = cx.theme().clone();
            let mut column = gpui_kit::component::v_flex().gap_2().p_3().min_w(px(280.));
            column = column.child(Label::new(strings::UI_DOWNLOADS).text_sm());
            if rows.is_empty() {
                column = column.child(
                    Label::new(strings::UI_NO_DOWNLOADS)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                );
            }
            // One row per in-flight download: every label and the bar need an
            // id unique to the row, or N downloads collide under one parent.
            for (index, (title, percent, detail, eta)) in rows.into_iter().enumerate() {
                column = column.child(
                    gpui_kit::component::v_flex()
                        .gap_1()
                        .child(
                            Label::new(title)
                                .id(SharedString::from(format!("download-{index}-title")))
                                .text_xs(),
                        )
                        .child(
                            Progress::new(SharedString::from(format!("download-{index}-bar")))
                                .value(percent as f32)
                                .color(theme.progress_bar)
                                .h(px(6.)),
                        )
                        .child(
                            Label::new(match eta {
                                Some(eta) => format!("{detail}  ·  {eta}"),
                                None => detail,
                            })
                            .id(SharedString::from(format!("download-{index}-detail")))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                        ),
                );
            }
            column.into_any_element()
        })
        .into_any_element()
}

/// Whether the offline banner should show above everything (`F-CAT-01`).
pub(crate) fn offline_banner(workspace: &Workspace, theme: &Theme) -> Option<AnyElement> {
    if workspace.catalog().status != CatalogStatus::Offline {
        return None;
    }
    Some(IntoElement::into_any_element(
        h_flex()
            .id("offline-banner")
            .w_full()
            .gap_2()
            .px(px(12.))
            .py(px(4.))
            .bg(theme.danger)
            .text_color(theme.danger_foreground)
            .text_xs()
            .child(icon(IconName::WifiOff, 14.0, theme.danger_foreground))
            .child(Label::new(strings::CATALOG_OFFLINE)),
    ))
}

/// The stale-catalog banner shown at the top of the centre pane (`F-CAT-01`).
pub(crate) fn stale_banner(workspace: &Workspace, theme: &Theme) -> Option<AnyElement> {
    if workspace.catalog().status != CatalogStatus::Stale {
        return None;
    }
    Some(IntoElement::into_any_element(
        h_flex()
            .id("catalog-stale-banner")
            .w_full()
            .gap_2()
            .items_center()
            .px(px(12.))
            .py(px(4.))
            .bg(crate::theme::tokens::banner_fill(theme.warning))
            .border_b_1()
            .border_color(crate::theme::tokens::banner_border(theme.warning))
            .text_xs()
            .child(icon(IconName::Info, 14.0, theme.warning))
            .child(Label::new(strings::CATALOG_STALE.to_owned()).text_color(theme.warning)),
    ))
}
