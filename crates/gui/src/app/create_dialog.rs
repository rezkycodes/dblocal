//! The create-instance modal (`F-INS-01`, `F-CAT-02`, `F-PRT-01`, `F-CRD-03`).
//!
//! The prototype's layout: engine cards with a monogram and the canonical port,
//! a version list from the catalog read model with the "Installed" / "Download,
//! size" badges, name and port fields, the superuser name, the no-password
//! option with its warning, and a footer that only promises a download when one
//! is really needed. Unavailable engines and versions are shown as
//! "not available on this platform", never hidden (`F-CAT-02` AC2).
//!
//! Validation goes through the service's `validate_spec`, so the form and the
//! real backend agree on every rule.

use dblocal_core::model::AuthMode;
use dblocal_core::service::EngineEntry;
use dblocal_core::strings;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Theme, h_flex, input::Input, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    Styled, Window, div, prelude::*, px,
};

use crate::format;

use super::Workspace;
use super::components::{engine_badge, icon, section_label};

/// Renders the create-instance modal when a form is open.
///
/// `window` is threaded in so each engine card and version row can take a
/// keyboard focus handle and draw the theme's focus ring when focused
/// (`F-PLH-01` AC1: reach any choice by Tab, confirm with Enter/Space).
pub(super) fn render(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let Some(form) = workspace.create_form().cloned() else {
        return div().into_any_element();
    };
    let catalog = workspace.catalog().clone();
    let entries = catalog.engines.clone();
    let selected_entry = entries
        .iter()
        .find(|entry| entry.engine == form.engine)
        .cloned();
    let installed = selected_entry
        .as_ref()
        .and_then(|entry| {
            entry
                .versions
                .iter()
                .find(|version| version.version == form.version)
        })
        .map(|version| version.installed)
        .unwrap_or(false);

    let body = v_flex()
        .gap_5()
        .p(px(20.))
        .text_size(px(13.))
        .child(Label::new(strings::UI_CREATE_INTRO).text_color(theme.muted_foreground))
        .child(
            v_flex()
                .gap_2()
                .child(section_label(strings::UI_FIELD_ENGINE).text_color(theme.muted_foreground))
                .child(render_category_tabs(&form, &theme, window, cx))
                .child(render_engine_cards(&entries, &form, &theme, window, cx)),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    section_label(format::create_versions(form.engine.display_name()))
                        .text_color(theme.muted_foreground),
                )
                .child(render_versions(
                    selected_entry.as_ref(),
                    &form,
                    &theme,
                    window,
                    cx,
                )),
        )
        .child(render_fields(workspace, &form, &theme, cx))
        .child(render_engine_options(&form, &theme, cx));

    let footer = render_footer(&form, installed, &theme, cx);
    overlay(
        "create-overlay",
        "create-panel",
        div()
            .w(px(720.))
            .max_h(px(760.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .overflow_hidden()
            .child(
                h_flex()
                    .px(px(20.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Label::new(strings::UI_CREATE_TITLE)
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD),
                    ),
            )
            .child(
                div()
                    .id("create-body")
                    .max_h(px(620.))
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(footer),
        workspace,
        window,
        cx,
    )
}

/// Category tabs: Database, Cache & Queue, Storage.
///
/// Every tab is always selectable; on a platform where all engines in a
/// category are unavailable (e.g. Cache & Queue on Windows), the tab still
/// opens and shows those engines greyed with their reason, never empty or hidden.
fn render_category_tabs(
    form: &super::CreateForm,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let tabs = format::EngineCategory::ALL.into_iter().map(|category| {
        let active = form.category == category;
        let focus = form
            .category_focus
            .iter()
            .find(|(cat, _)| *cat == category)
            .map(|(_, handle)| handle.clone());
        let tab = div()
            .id(SharedString::from(format!("category-tab-{}", category.slug())))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .border_1()
            .cursor_pointer()
            .border_color(if active { theme.primary } else { theme.border })
            .bg(if active {
                crate::theme::tokens::tint(theme.primary, 0.12)
            } else {
                theme.background
            })
            .hover(|s| s.border_color(theme.primary))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose_category_action(category, window, cx);
            }))
            .child(
                Label::new(category.label())
                    .id(SharedString::from(format!(
                        "category-label-{}",
                        category.slug()
                    )))
                    .text_xs()
                    .font_weight(if active {
                        gpui_kit::FontWeight::SEMIBOLD
                    } else {
                        gpui_kit::FontWeight::MEDIUM
                    })
                    .text_color(if active {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    }),
            );
        match focus {
            Some(focus) => super::components::apply_focus_ring(tab, &focus, window, cx),
            None => tab,
        }
    });
    h_flex().gap_2().children(tabs)
}

/// The engine cards for the active category tab: icon, name, canonical port, and the disabled reason.
fn render_engine_cards(
    entries: &[EngineEntry],
    form: &super::CreateForm,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let cards = crate::format::category_engine_cards(entries, form.category);
    // One focus handle per engine card, taken from the form so the keyboard
    // tests focus the exact same control the view draws (`F-PLH-01` AC1).
    let cards = cards.into_iter().map(|entry| {
        let engine = entry.engine;
        let focus = form
            .engine_focus
            .iter()
            .find(|(key, _)| *key == engine)
            .map(|(_, handle)| handle.clone());
        let selected = engine == form.engine;
        let disabled = !entry.available;
        let reason = format::engine_unavailable_reason(entry).map(str::to_owned);
        let card = v_flex()
            .id(SharedString::from(format!("engine-{}", engine.as_str())))
            .flex_1()
            .min_w(px(120.))
            .gap_2()
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .cursor_pointer()
            .border_color(if selected {
                theme.primary
            } else {
                theme.border
            })
            .bg(if selected {
                crate::theme::tokens::tint(theme.primary, 0.08)
            } else {
                theme.background
            })
            .hover(|s| s.border_color(theme.primary))
            .on_click(cx.listener(move |this, _, window, cx| {
                if !disabled {
                    this.choose_engine_action(engine, window, cx);
                }
            }))
            .child(
                h_flex()
                    .items_center()
                    .child(engine_badge(engine, 28.0, theme))
                    .child(div().flex_1())
                    .when(selected, |this| {
                        this.child(icon(IconName::Check, 16.0, theme.primary))
                    }),
            )
            .child(Label::new(engine.display_name()).font_weight(gpui_kit::FontWeight::SEMIBOLD))
            .child(
                Label::new(format!("Canonical port {}", engine.canonical_port()))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            )
            .when_some(reason, |this, reason| {
                this.child(
                    Label::new(reason)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
            });
        match (disabled, focus) {
            (false, Some(focus)) => super::components::apply_focus_ring(card, &focus, window, cx),
            _ => card,
        }
    });
    h_flex().gap_2p5().flex_wrap().children(cards)
}

/// The version list with the "Only show latest versions" toggle, status badges and
/// the installed / download badges.
fn render_versions(
    entry: Option<&EngineEntry>,
    form: &super::CreateForm,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = theme.clone();
    let toggle = latest_only_toggle(form.latest_only, &theme, cx);
    let mut list = v_flex()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            div()
                .px(px(14.))
                .py(px(8.))
                .border_b_1()
                .border_color(theme.table_row_border)
                .child(toggle),
        );
    let Some(entry) = entry else {
        return list.child(
            Label::new(strings::CATALOG_ENGINE_UNAVAILABLE_PLATFORM)
                .text_xs()
                .p_3()
                .text_color(theme.muted_foreground),
        );
    };
    let unavailable = format::version_unavailable_reason(entry).map(str::to_owned);
    let rows = format::picker_versions(entry, form.version_filter());
    for version in &rows {
        let focus = form
            .version_focus
            .iter()
            .find(|(tag, _)| tag == &version.version)
            .map(|(_, handle)| handle.clone());
        let selected = version.version == form.version;
        let tag = version.version.clone();
        let version_unavailable = unavailable.is_some();
        let download = if version_unavailable {
            None
        } else if version.installed {
            Some((strings::UI_VERSION_INSTALLED.to_owned(), theme.success))
        } else {
            Some((
                format::create_download_badge(&format::format_bytes(version.size_bytes)),
                theme.muted_foreground,
            ))
        };
        let mut row = h_flex()
            .id(SharedString::from(format!("version-{tag}")))
            .h(px(42.))
            .px(px(14.))
            .gap_3()
            .cursor_pointer()
            .border_t_1()
            .border_color(theme.table_row_border)
            .bg(if selected {
                crate::theme::tokens::tint(theme.primary, 0.10)
            } else {
                theme.background
            })
            .hover(|s| s.bg(theme.muted))
            .on_click(cx.listener(move |this, _, window, cx| {
                if !version_unavailable {
                    this.choose_version_action(tag.clone(), window, cx);
                }
            }))
            .child(radio_marker(selected, version_unavailable, &theme))
            .child(
                Label::new(version.version.clone())
                    .w(px(90.))
                    .font_weight(gpui_kit::FontWeight::MEDIUM),
            );
        // Status first (LTS / Innovation / Rolling), then EOL, so the support
        // level reads before the warning without duplicating EOL when status is
        // None.
        for text in format::version_status_badges(version) {
            let colour = if text == strings::UI_VERSION_EOL {
                theme.danger
            } else {
                status_badge_colour(version, &theme)
            };
            row = row.child(
                Label::new(text)
                    .px_2()
                    .rounded_full()
                    .bg(theme.muted)
                    .text_xs()
                    .text_color(colour),
            );
        }
        row = row
            .child(div().flex_1())
            .when_some(download, |this, (text, colour)| {
                this.child(
                    Label::new(text)
                        .px_2()
                        .rounded_full()
                        .bg(theme.muted)
                        .text_xs()
                        .text_color(colour),
                )
            });
        row = row.when_some(unavailable.clone(), |this, reason| {
            this.child(
                Label::new(reason)
                    .text_xs()
                    .text_color(theme.muted_foreground),
            )
        });
        list = list.child(match (version_unavailable, focus) {
            (false, Some(focus)) => {
                super::components::apply_inset_focus_ring(row, &focus, window, cx)
            }
            _ => row,
        });
    }
    list
}

/// The "Only show latest versions" checkbox row shown above the version list.
fn latest_only_toggle(
    checked: bool,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    gpui_kit::component::checkbox::Checkbox::new("latest-only")
        .label(strings::UI_CREATE_LATEST_ONLY)
        .checked(checked)
        .on_click(cx.listener(|this, checked: &bool, window, cx| {
            this.set_latest_only_action(*checked, window, cx);
        }))
        .text_color(theme.muted_foreground)
}

/// The colour of a row's support-status badge.
fn status_badge_colour(
    version: &dblocal_core::service::VersionEntry,
    theme: &Theme,
) -> gpui_kit::Hsla {
    use dblocal_core::model::VersionStatus;
    match version.status {
        Some(VersionStatus::Lts) => theme.primary,
        Some(VersionStatus::Innovation) => theme.info,
        Some(VersionStatus::Rolling) => theme.success,
        _ => theme.muted_foreground,
    }
}

/// The small radio dot on each version row.
fn radio_marker(selected: bool, disabled: bool, theme: &Theme) -> impl IntoElement {
    let border = if disabled {
        theme.border
    } else if selected {
        theme.primary
    } else {
        theme.muted_foreground
    };
    div()
        .flex_none()
        .size(px(16.))
        .rounded_full()
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |this| {
            this.child(div().size(px(8.)).rounded_full().bg(theme.primary))
        })
}

/// Name, port and superuser fields with the prototype's inline hints.
fn render_fields(
    workspace: &Workspace,
    form: &super::CreateForm,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let _ = cx;
    let is_rustfs = form.engine == dblocal_core::model::EngineKey::Rustfs;
    let superuser_label = if is_rustfs {
        "Access key"
    } else {
        strings::UI_OVERVIEW_SUPERUSER
    };
    let superuser = if is_rustfs {
        "Auto-generated"
    } else {
        form.engine.default_superuser()
    };
    let superuser_hint = if is_rustfs {
        "Random per-instance key"
    } else {
        strings::UI_CREATE_SUPERUSER_HINT
    };
    h_flex()
        .gap_3()
        .items_start()
        .child(
            v_flex()
                .flex_1()
                .gap_1p5()
                .child(Label::new(strings::UI_FIELD_NAME).font_weight(gpui_kit::FontWeight::MEDIUM))
                .child(input_or_empty(form.name_input.as_ref()))
                .child(
                    Label::new(strings::UI_CREATE_NAME_HINT)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        )
        .child(
            v_flex()
                .w(px(160.))
                .gap_1p5()
                .child(Label::new(strings::UI_FIELD_PORT).font_weight(gpui_kit::FontWeight::MEDIUM))
                .child(input_or_empty(form.port_input.as_ref()))
                .child(
                    Label::new(strings::UI_CREATE_PORT_HINT)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        )
        .child(
            v_flex()
                .w(px(140.))
                .gap_1p5()
                .child(
                    Label::new(superuser_label)
                        .font_weight(gpui_kit::FontWeight::MEDIUM),
                )
                .child(
                    div()
                        .h(px(32.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .text_color(theme.muted_foreground)
                        .child(Label::new(superuser.to_owned())),
                )
                .child(
                    Label::new(superuser_hint)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                ),
        )
        .when_some(workspace.create_form(), |this, _| this)
}

/// Engine-specific create options: RustFS offers the optional web console
/// checkbox (OFF by default); other engines offer the no-password option.
fn render_engine_options(
    form: &super::CreateForm,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    if form.engine == dblocal_core::model::EngineKey::Rustfs {
        let enabled = form.console_enabled;
        v_flex()
            .gap_1()
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(if enabled { theme.primary } else { theme.border })
            .bg(if enabled {
                crate::theme::tokens::tint(theme.primary, 0.08)
            } else {
                theme.background
            })
            .child(
                gpui_kit::component::checkbox::Checkbox::new("rustfs-console")
                    .label("Enable web console")
                    .checked(enabled)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_console_enabled_action(*checked);
                        cx.notify();
                    })),
            )
            .child(
                Label::new(
                    "Starts the RustFS web console on a second loopback port (default 9001 when free). Off by default.",
                )
                .text_xs()
                .text_color(theme.muted_foreground)
                .pl(px(24.)),
            )
            .into_any_element()
    } else {
        render_no_password(form, theme, cx).into_any_element()
    }
}

/// The no-password option and its warning border (`F-CRD-03` AC1).
fn render_no_password(
    form: &super::CreateForm,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let active = form.auth == AuthMode::None;
    v_flex()
        .gap_1()
        .p(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(if active {
            crate::theme::tokens::tint(theme.danger, 0.5)
        } else {
            theme.border
        })
        .bg(if active {
            crate::theme::tokens::tint(theme.danger, 0.08)
        } else {
            theme.background
        })
        .child(
            gpui_kit::component::checkbox::Checkbox::new("no-auth")
                .label(strings::UI_CREATE_NO_PASSWORD)
                .checked(active)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.choose_auth_action(if *checked {
                        AuthMode::None
                    } else {
                        AuthMode::Password
                    });
                    cx.notify();
                })),
        )
        .child(
            Label::new(strings::UI_CREATE_NO_PASSWORD_WARNING)
                .text_xs()
                .text_color(theme.muted_foreground)
                .pl(px(24.)),
        )
}

/// The footer: the download note only when one is needed, then Cancel / Create.
fn render_footer(
    form: &super::CreateForm,
    installed: bool,
    theme: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let note = format::create_footer_note(installed);
    let note_colour = if installed {
        theme.success
    } else {
        theme.muted_foreground
    };
    h_flex()
        .gap_3()
        .px(px(20.))
        .py(px(12.))
        .border_t_1()
        .border_color(theme.border)
        .child(Label::new(note).flex_1().text_xs().text_color(note_colour))
        .child(
            Button::new("ni-cancel")
                .label(strings::UI_CANCEL)
                .track_focus(&form.cancel_focus)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_create_action();
                    cx.notify();
                })),
        )
        .child(
            Button::new("ni-create")
                .primary()
                .label(strings::UI_CREATE_SUBMIT_PROTOTYPE)
                .track_focus(&form.submit_focus)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.submit_create_action(window, cx);
                })),
        )
}

/// The input, or an empty spacer when a headless preview has no entity.
///
/// The inputs use the default height (32 logical px), the same as the
/// superuser box beside them, as the prototype draws the row.
fn input_or_empty(
    input: Option<&gpui_kit::Entity<gpui_kit::component::input::InputState>>,
) -> AnyElement {
    match input {
        Some(input) => Input::new(input).into_any_element(),
        None => div().h(px(32.)).into_any_element(),
    }
}

/// Shared modal chrome: a full-screen backdrop that closes on click, a centred
/// panel that swallows clicks, and a **focus trap** (`F2`).
///
/// The panel registers a `gpui-base` focus trap on a per-overlay keyed handle,
/// so when it holds focus, `Tab`/`Shift+Tab` cycle inside the dialog only — the
/// modal no longer lets Tab reach the header, sidebar and status bar behind its
/// scrim. `window` is needed for the keyed handle.
pub(super) fn overlay(
    overlay_id: &'static str,
    panel_id: &'static str,
    panel: impl IntoElement,
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    use gpui_kit::component::FocusTrapElement as _;
    // The modal's container handle comes from `begin_modal` so `open_*` can focus
    // it on open; fall back to a keyed handle if a preview drew the dialog
    // without going through an open path.
    let container = workspace.modal_focus().unwrap_or_else(|| {
        window
            .use_keyed_state(
                SharedString::from(format!("modal-focus-fallback-{overlay_id}")),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone()
    });
    gpui_kit::deferred(
        div()
            .id(overlay_id)
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .bg(gpui_kit::hsla(0.0, 0.0, 0.0, 0.45))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_modals_action();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id(panel_id)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(panel)
                    .focus_trap(SharedString::from(format!("trap-{overlay_id}")), &container),
            ),
    )
    .with_priority(10)
    .into_any_element()
}
