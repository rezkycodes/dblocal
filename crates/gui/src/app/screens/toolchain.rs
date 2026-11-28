//! Toolchain version pages (PHP, Node.js, Composer), the shared PATH panel,
//! and the PHP settings card (v0.3.0).
//!
//! Pure rendering over [`crate::app::toolchain_state::ToolchainState`]: all
//! version ordering, status derivation, update detection, button enablement,
//! PATH health classification and PHP settings validation live in
//! `toolchain_state.rs`. Every paint comes from the active theme or
//! [`crate::theme::tokens`] (zero colour literals).

use dblocal_core::toolchain::catalog::{Profile, ToolId};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Window, div, px,
};

use crate::format;
use crate::theme::tokens;

use super::Workspace;
use super::page;
use crate::app::components::{banner, icon, row_destructive_variant, toolchain_badge};
use crate::app::design;
use crate::app::toolchain_state::{
    ToolchainVersionRow, VersionRowStatus, profile_description, tool_display_name, tool_subtitle,
};

impl Workspace {
    /// Renders the full toolchain page for `tool` inside the centre pane.
    pub(crate) fn render_toolchain_page(
        &self,
        tool: ToolId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let rows = self.toolchain.version_rows(tool);

        let mut content = page().debug_selector(|| format!("toolchain-page-{}", tool.as_str()));

        // Composer warning when no PHP is active.
        if tool == ToolId::Composer && self.toolchain.composer_needs_php() {
            content = content.child(self.render_composer_needs_php_banner(&theme, cx));
        }

        // Offline / empty catalog banner.
        if self.toolchain.catalog_unavailable(tool) {
            content = content.child(self.render_catalog_unavailable_banner(tool, &theme, cx));
        }

        // First-launch empty state callout when nothing of this tool is installed.
        if let Some(recommended) = self.toolchain.recommended_first_install(tool) {
            content = content.child(self.render_first_install_card(tool, &recommended, &theme, cx));
        }

        // Linux PHP extension profile selector.
        if self.toolchain.show_php_profile_selector(tool) {
            content = content.child(self.render_php_profile_selector(&theme, cx));
        }

        // Versions list card.
        content = content.child(self.render_toolchain_versions_card(tool, &rows, &theme, cx));

        // PHP settings card (PHP page only).
        if tool == ToolId::Php {
            content = content.child(self.render_php_settings_card(&theme, window, cx));
        }

        // Shared PATH panel (always reachable on every toolchain page).
        content = content.child(self.render_path_panel(&theme, cx));

        v_flex()
            .id("toolchain-center")
            .flex_1()
            .h_full()
            .min_w_0()
            .bg(theme.background)
            .child(self.render_toolchain_header(tool, &rows, &theme, cx))
            .child(
                div()
                    .id("toolchain-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }

    /// Header: framed toolchain badge, tool name, active version pill, subtitle,
    /// and a "Make active" or refresh control.
    fn render_toolchain_header(
        &self,
        tool: ToolId,
        rows: &[ToolchainVersionRow],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.toolchain.active_version(tool);
        let active_pill = match active {
            Some(a) => {
                let text = match a.profile() {
                    Some(p) if tool == ToolId::Php => format!("Active {} ({p})", a.version()),
                    _ => format!("Active {}", a.version()),
                };
                design::chip(&format!("tc-active-{}", tool.as_str()), text, theme.success)
            }
            None => design::chip(
                &format!("tc-inactive-{}", tool.as_str()),
                "No active version",
                theme.muted_foreground,
            ),
        };

        // When installed versions exist but none is active, offer a quick
        // "Make active" control in the header targeting the newest installed.
        let quick_activate = if active.is_none() {
            rows.iter()
                .find(|r| r.can_activate())
                .map(|r| r.key.version.clone())
        } else {
            None
        };

        h_flex()
            .id("toolchain-header")
            .debug_selector(|| "toolchain-header".to_owned())
            .flex_wrap()
            .gap_4()
            .px(px(24.))
            .pt(px(18.))
            .pb(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .items_center()
            .child(
                div()
                    .p(px(3.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .child(toolchain_badge(tool, 40.0, theme)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .gap_0p5()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2p5()
                            .items_center()
                            .child(
                                Label::new(tool_display_name(tool))
                                    .id(SharedString::from(format!("tc-title-{}", tool.as_str())))
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(active_pill),
                    )
                    .child(
                        Label::new(tool_subtitle(tool))
                            .id(SharedString::from(format!("tc-sub-{}", tool.as_str())))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_none()
                    .when_some(quick_activate, |bar, ver| {
                        let v_click = ver.clone();
                        bar.child(
                            Button::new("tc-header-activate")
                                .primary()
                                .small()
                                .icon(IconName::Check)
                                .label(format!("Make {ver} active"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.activate_toolchain_version(tool, v_click.clone(), cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("tc-header-refresh")
                            .outline()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_toolchains_bg(cx);
                            })),
                    ),
            )
    }

    /// Banner shown on the Composer page when no PHP version is active.
    fn render_composer_needs_php_banner(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        banner(theme.warning)
            .id("tc-composer-needs-php")
            .debug_selector(|| "tc-composer-needs-php".to_owned())
            .items_center()
            .child(icon(IconName::TriangleAlert, 16.0, theme.warning))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(200.))
                    .gap_0p5()
                    .child(
                        Label::new("Composer requires an active PHP version")
                            .id("tc-composer-needs-php-title")
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(
                            "The composer shim runs composer.phar through your active PHP shim. \
                             Install and activate a PHP version so `composer` can execute.",
                        )
                        .id("tc-composer-needs-php-body")
                        .text_xs()
                        .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                Button::new("tc-composer-goto-php")
                    .outline()
                    .small()
                    .label("Go to PHP")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.select_toolchain(ToolId::Php, cx);
                    })),
            )
            .into_any_element()
    }

    /// Banner shown when the toolchain catalog is empty or offline for `tool`.
    fn render_catalog_unavailable_banner(
        &self,
        tool: ToolId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        banner(theme.warning)
            .id("tc-catalog-unavailable")
            .debug_selector(|| "tc-catalog-unavailable".to_owned())
            .items_center()
            .child(icon(IconName::TriangleAlert, 16.0, theme.warning))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(200.))
                    .gap_0p5()
                    .child(
                        Label::new("Catalog unavailable")
                            .id("tc-catalog-unavailable-title")
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(format!(
                            "No downloadable {} builds are available in the current catalog for \
                             this platform. Any already-installed versions remain usable.",
                            tool_display_name(tool)
                        ))
                        .id("tc-catalog-unavailable-body")
                        .text_xs()
                        .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                Button::new("tc-catalog-retry")
                    .outline()
                    .small()
                    .label("Retry")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.refresh_toolchains_bg(cx);
                    })),
            )
            .into_any_element()
    }

    /// First-launch empty state callout with a single clear next action.
    fn render_first_install_card(
        &self,
        tool: ToolId,
        recommended: &ToolchainVersionRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ver = recommended.key.version.clone();
        let prof = recommended.key.profile;
        let busy = recommended.busy;
        let name = tool_display_name(tool);
        h_flex()
            .id("tc-empty-cta")
            .flex_wrap()
            .gap_3()
            .p(px(14.))
            .rounded(px(design::GROUP_RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(tokens::card(theme))
            .items_center()
            .child(design::icon_tile(IconName::Download, 34.0, theme.primary, theme))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(200.))
                    .gap_0p5()
                    .child(
                        Label::new(format!("No {name} versions installed yet"))
                            .id("tc-empty-cta-title")
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(format!(
                            "Install {name} {ver} ({}) — it will be activated automatically as your first version.",
                            format::format_bytes(recommended.size_bytes)
                        ))
                        .id("tc-empty-cta-body")
                        .text_xs()
                        .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                Button::new("tc-empty-install-btn")
                    .primary()
                    .small()
                    .icon(IconName::Download)
                    .label(format!("Install {name} {ver}"))
                    .disabled(busy)
                    .loading(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.install_toolchain_version(tool, ver.clone(), prof, cx);
                    })),
            )
            .into_any_element()
    }

    /// Linux PHP extension profile selector (`Common` / `Bulk`).
    fn render_php_profile_selector(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.toolchain.php_profile;
        let common_active = selected == Profile::Common;
        let bulk_active = selected == Profile::Bulk;

        v_flex()
            .id("tc-php-profile")
            .gap_1p5()
            .child(design::group_heading("Extension profile", theme))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .px(px(design::ROW_PAD_X))
                    .py(px(10.))
                    .rounded(px(design::GROUP_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .bg(tokens::card(theme))
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .flex_none()
                            .child(
                                Button::new("tc-profile-common")
                                    .small()
                                    .when(common_active, |b| b.primary())
                                    .when(!common_active, |b| b.outline())
                                    .label("Common (default)")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_php_profile(Profile::Common, cx);
                                    })),
                            )
                            .child(
                                Button::new("tc-profile-bulk")
                                    .small()
                                    .when(bulk_active, |b| b.primary())
                                    .when(!bulk_active, |b| b.outline())
                                    .label("Bulk")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_php_profile(Profile::Bulk, cx);
                                    })),
                            ),
                    )
                    .child(
                        Label::new(profile_description(selected))
                            .id("tc-profile-desc")
                            .flex_1()
                            .min_w(px(220.))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .into_any_element()
    }

    /// The Versions grouped card.
    fn render_toolchain_versions_card(
        &self,
        tool: ToolId,
        rows: &[ToolchainVersionRow],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = design::group_card(&format!("tc-versions-{}", tool.as_str()), theme)
            .debug_selector(|| format!("tc-versions-card-{}", tool.as_str()));

        if rows.is_empty() {
            card = card.child(
                Label::new("No versions available for this platform.")
                    .id("tc-versions-empty")
                    .px(px(design::ROW_PAD_X))
                    .py(px(14.))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        } else {
            for (idx, row) in rows.iter().enumerate() {
                card = card.child(self.render_version_row(idx, row, idx == 0, theme, cx));
            }
        }

        v_flex()
            .gap_1p5()
            .child(design::group_heading("Versions", theme))
            .child(card)
            .into_any_element()
    }

    /// One version row inside the Versions card.
    fn render_version_row(
        &self,
        idx: usize,
        row: &ToolchainVersionRow,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono = theme.mono_font_family.clone();
        let tool = row.key.tool;
        let version = row.key.version.clone();
        let profile = row.key.profile;
        let row_key = row.key.clone();

        let status_chip = match row.status {
            VersionRowStatus::Active => {
                design::chip(&format!("tc-status-{idx}"), "Active", theme.success)
            }
            VersionRowStatus::Installed => {
                design::chip(&format!("tc-status-{idx}"), "Installed", theme.primary)
            }
            VersionRowStatus::Available => design::chip(
                &format!("tc-status-{idx}"),
                "Available",
                theme.muted_foreground,
            ),
        };

        let mut meta_line = h_flex().flex_wrap().gap_2().items_center().child(
            Label::new(format::format_bytes(row.size_bytes))
                .id(SharedString::from(format!("tc-size-{idx}")))
                .text_xs()
                .text_color(theme.muted_foreground),
        );
        if let Some(p) = profile {
            meta_line = meta_line.child(
                Label::new(format!("· {}", p.as_str()))
                    .id(SharedString::from(format!("tc-prof-{idx}")))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }
        if row.missing_pdo_pgsql_note {
            meta_line = meta_line.child(
                Label::new("· pdo_pgsql not included in this build")
                    .id(SharedString::from(format!("tc-no-pgsql-{idx}")))
                    .text_xs()
                    .text_color(theme.warning),
            );
        }
        if !row.extensions.is_empty() {
            let toggle_key = row_key.clone();
            let toggle_label = if row.extensions_expanded {
                format!("Hide extensions ({})", row.extensions.len())
            } else {
                format!("Extensions ({})", row.extensions.len())
            };
            meta_line = meta_line.child(
                Button::new(SharedString::from(format!("tc-ext-toggle-{idx}")))
                    .ghost()
                    .xsmall()
                    .compact()
                    .label(toggle_label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_php_extensions(toggle_key.clone(), cx);
                    })),
            );
        }

        let left = v_flex()
            .flex_1()
            .min_w(px(180.))
            .gap_0p5()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(
                        Label::new(version.clone())
                            .id(SharedString::from(format!("tc-ver-{idx}")))
                            .font_family(mono.clone())
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .when(row.lts, |r| {
                        r.child(design::chip(&format!("tc-lts-{idx}"), "LTS", theme.info))
                    })
                    .child(status_chip)
                    .when_some(row.update_available.as_ref(), |r, latest| {
                        r.child(design::chip(
                            &format!("tc-upd-{idx}"),
                            format!("Update {latest}"),
                            theme.warning,
                        ))
                    }),
            )
            .child(meta_line);

        let right = self.render_version_row_actions(idx, row, theme, cx);

        let mut container = v_flex()
            .id(SharedString::from(format!("tc-row-{idx}")))
            .debug_selector(move || format!("tc-row-{idx}"))
            .px(px(design::ROW_PAD_X))
            .py(px(10.))
            .gap_2()
            .when(!first, |d| {
                d.border_t_1().border_color(tokens::hairline(theme))
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(left)
                    .child(right),
            );

        // Expandable extension list for PHP builds.
        if row.extensions_expanded && !row.extensions.is_empty() {
            let ext_text = row.extensions.join(", ");
            container = container.child(
                div()
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        Label::new(ext_text)
                            .id(SharedString::from(format!("tc-ext-list-{idx}")))
                            .text_xs()
                            .font_family(mono)
                            .text_color(theme.muted_foreground),
                    ),
            );
        }

        // Inline error with retry button (never swallowed).
        if let Some(err_msg) = &row.error {
            let retry_ver = version;
            container = container.child(
                banner(theme.danger)
                    .items_center()
                    .child(icon(IconName::TriangleAlert, 14.0, theme.danger))
                    .child(
                        Label::new(err_msg.clone())
                            .id(SharedString::from(format!("tc-err-{idx}")))
                            .flex_1()
                            .min_w(px(160.))
                            .text_xs()
                            .text_color(theme.danger),
                    )
                    .child(
                        Button::new(SharedString::from(format!("tc-retry-{idx}")))
                            .outline()
                            .xsmall()
                            .compact()
                            .label("Retry")
                            .disabled(row.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.install_toolchain_version(
                                    tool,
                                    retry_ver.clone(),
                                    profile,
                                    cx,
                                );
                            })),
                    ),
            );
        }

        container.into_any_element()
    }

    /// Right-aligned action controls for one version row.
    fn render_version_row_actions(
        &self,
        idx: usize,
        row: &ToolchainVersionRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tool = row.key.tool;
        let version = row.key.version.clone();
        let profile = row.key.profile;

        if let Some(progress) = &row.progress {
            let pct = progress.percent().unwrap_or(15) as f32;
            let cancel_key = row.key.clone();
            return h_flex()
                .flex_none()
                .gap_2()
                .items_center()
                .child(
                    v_flex()
                        .w(px(140.))
                        .gap_1()
                        .child(
                            Label::new(progress.label())
                                .id(SharedString::from(format!("tc-prog-lbl-{idx}")))
                                .text_xs()
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(5.))
                                .rounded_full()
                                .bg(tokens::active(theme))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(140.0 * (pct / 100.0)))
                                        .rounded_full()
                                        .bg(theme.primary),
                                ),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("tc-cancel-{idx}")))
                        .outline()
                        .small()
                        .compact()
                        .label("Cancel")
                        .disabled(!row.can_cancel())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cancel_toolchain_install(&cancel_key, cx);
                        })),
                )
                .into_any_element();
        }

        let mut bar = h_flex().flex_none().gap_1p5().items_center();

        if let Some(latest) = row.update_available.clone() {
            bar = bar.child(
                Button::new(SharedString::from(format!("tc-update-{idx}")))
                    .outline()
                    .small()
                    .icon(IconName::Download)
                    .label("Update")
                    .disabled(!row.can_update())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.install_toolchain_version(tool, latest.clone(), profile, cx);
                    })),
            );
        }

        match row.status {
            VersionRowStatus::Available => {
                let v = version.clone();
                bar = bar.child(
                    Button::new(SharedString::from(format!("tc-install-{idx}")))
                        .outline()
                        .small()
                        .icon(IconName::Download)
                        .label("Install")
                        .disabled(!row.can_install())
                        .loading(row.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.install_toolchain_version(tool, v.clone(), profile, cx);
                        })),
                );
            }
            VersionRowStatus::Installed => {
                let v_act = version.clone();
                let v_rem = version;
                bar = bar
                    .child(
                        Button::new(SharedString::from(format!("tc-activate-{idx}")))
                            .primary()
                            .small()
                            .label("Make active")
                            .disabled(!row.can_activate())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.activate_toolchain_version(tool, v_act.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("tc-remove-{idx}")))
                            .custom(row_destructive_variant(theme, cx))
                            .small()
                            .compact()
                            .icon(IconName::Trash)
                            .tooltip("Remove version")
                            .accessibility_label(format!(
                                "Remove {} {v_rem}",
                                tool_display_name(tool)
                            ))
                            .disabled(!row.can_remove())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_toolchain_remove_dialog(
                                    tool,
                                    v_rem.clone(),
                                    profile,
                                    window,
                                    cx,
                                );
                            })),
                    );
            }
            VersionRowStatus::Active => {
                let v_rem = version;
                bar = bar.child(
                    Button::new(SharedString::from(format!("tc-remove-{idx}")))
                        .custom(row_destructive_variant(theme, cx))
                        .small()
                        .compact()
                        .icon(IconName::Trash)
                        .tooltip("Remove active version")
                        .accessibility_label(format!("Remove {} {v_rem}", tool_display_name(tool)))
                        .disabled(!row.can_remove())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_toolchain_remove_dialog(
                                tool,
                                v_rem.clone(),
                                profile,
                                window,
                                cx,
                            );
                        })),
                );
            }
        }

        bar.into_any_element()
    }
}
