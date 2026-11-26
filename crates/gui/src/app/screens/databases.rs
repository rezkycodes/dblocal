//! The Databases tab (`F-DBU-01..03`), live over `DblService`.
//!
//! Three cards, exactly as the prototype draws them:
//!
//! 1. **Database** — a table of name / owner / size from `list_databases`, a
//!    name field and "Create database" (validated with the core allowlist before
//!    any round trip, `F-DBU-01` AC4), and a "Hapus" per row that opens a
//!    type-the-name confirmation (`F-DBU-01` AC3).
//! 2. **User & hak akses** — the roles `list_users` returned and a create-user
//!    form (name, database, read-only / read-write). The generated password is
//!    shown once in a dialog and never enters this view's state
//!    (`F-DBU-02` AC2).
//! 3. **Superuser password** — "Reset password" behind a confirmation; the new
//!    URI is shown once with Copy (`F-DBU-03` AC1). A failed reset keeps the
//!    old password working and says so (`F-DBU-03` AC2).
//!
//! Every control is a visible button; nothing is hover-only. All rows and
//! dialogs carry ids derived from the instance and the row name, so two
//! instances with a database of the same name cannot collide in the a11y tree.
//!
//! A stopped instance shows an empty state with a one-click Start, not a
//! connection error. An engine without a database/user model (Valkey) shows the
//! typed message the service returns; the view never invents text.

use dblocal_core::admin::PrivilegeLevel;
use dblocal_core::service::InstanceView;
use dblocal_core::strings;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, Theme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, div, px,
};

use super::Workspace;
use super::page;
use crate::app::admin_state::{AdminAction, level_label, row_id, row_part_id, user_label};
use crate::app::components::{card, icon, section_label};
use crate::format;

impl Workspace {
    pub(crate) fn render_databases(
        &self,
        view: &InstanceView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        // A stopped instance has no server to ask; the view offers a Start
        // rather than a connection error. A running engine that cannot do
        // admin (Valkey, or a PostgreSQL without a stored password) is not
        // special-cased: the service answers with a typed message and the
        // retry-able error card shows exactly that text, never an invented one.
        if !crate::format::is_running(view.status) {
            return page()
                .child(self.render_databases_stopped(view, &theme, cx))
                .into_any_element();
        }
        page()
            .child(self.render_databases_table(view, &theme, cx))
            .when_some(self.render_admin_error(view, &theme, cx), |this, error| {
                this.child(error)
            })
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(360.))
                            .child(self.render_users_card(view, &theme, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(300.))
                            .child(self.render_reset_card(view, &theme, cx)),
                    ),
            )
            .child(
                crate::app::components::banner(theme.muted_foreground)
                    .items_center()
                    .child(
                        Label::new(strings::UI_DATABASES_BOUNDARY)
                            .flex_1()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .into_any_element()
    }

    /// Empty state for a stopped instance: why the list is empty and a Start.
    fn render_databases_stopped(
        &self,
        view: &InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = view.id.clone();
        card(None, Some("databases-stopped"), theme)
            .child(Label::new(strings::UI_DATABASES_STOPPED).text_color(theme.muted_foreground))
            .child(
                Button::new(SharedString::from(row_part_id(
                    "databases-start",
                    &view.id,
                    "click",
                    "button",
                )))
                .label(strings::UI_DATABASES_START)
                .primary()
                .icon(gpui_kit::assets::IconName::Play)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.start_instance(id.clone(), cx);
                    cx.notify();
                })),
            )
    }

    /// The Database card: table, create row, per-row delete.
    fn render_databases_table(
        &self,
        view: &InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let admin = self.admin_state_for(&view.id);
        let rows = admin.map(|admin| admin.rows().to_vec()).unwrap_or_default();
        let busy = admin.is_some_and(|admin| admin.is_busy(AdminAction::CreateDatabase));
        let loading = admin.is_none_or(|admin| admin.pending());
        let mono = theme.mono_font_family.clone();
        let id_create = view.id.clone();

        let mut table = v_flex()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .child(
                h_flex()
                    .p(px(14.))
                    .gap_2()
                    .items_center()
                    .child(
                        Label::new(strings::databases_heading(rows.len()))
                            .id(SharedString::from(row_part_id(
                                "databases-heading",
                                &view.id,
                                "count",
                                "label",
                            )))
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        div()
                            .w(px(200.))
                            .child(Input::new(&self.db_name_input_entity()).small()),
                    )
                    .child(
                        Button::new(SharedString::from(row_part_id(
                            "databases-create",
                            &view.id,
                            "click",
                            "button",
                        )))
                        .primary()
                        .small()
                        .label(strings::UI_DATABASES_CREATE)
                        .loading(busy)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.create_database(&id_create, cx);
                        })),
                    ),
            )
            .child(
                h_flex()
                    .h(px(30.))
                    .px(px(14.))
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Label::new(strings::UI_DATABASES_COLUMN_NAME)
                            .id(SharedString::from(row_part_id(
                                "databases-col",
                                &view.id,
                                "name",
                                "header",
                            )))
                            .flex_1(),
                    )
                    .child(
                        Label::new(strings::UI_DATABASES_COLUMN_OWNER)
                            .id(SharedString::from(row_part_id(
                                "databases-col",
                                &view.id,
                                "owner",
                                "header",
                            )))
                            .w(px(140.)),
                    )
                    .child(
                        Label::new(strings::UI_DATABASES_COLUMN_SIZE)
                            .id(SharedString::from(row_part_id(
                                "databases-col",
                                &view.id,
                                "size",
                                "header",
                            )))
                            .w(px(100.)),
                    )
                    .child(div().w(px(80.))),
            );

        if loading {
            table = table.child(
                Label::new(strings::UI_DATABASES_LOADING)
                    .id(SharedString::from(row_part_id(
                        "databases-loading",
                        &view.id,
                        "row",
                        "label",
                    )))
                    .p(px(14.))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        } else if rows.is_empty() {
            table = table.child(
                Label::new(strings::UI_DATABASES_EMPTY)
                    .id(SharedString::from(row_part_id(
                        "databases-empty",
                        &view.id,
                        "row",
                        "label",
                    )))
                    .p(px(14.))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }

        for db in &rows {
            let name = db.name().to_owned();
            let row = row_id("db-row", &view.id, &name);
            let id_open = view.id.clone();
            let id_name = name.clone();
            let id_delete = view.id.clone();
            let id_del_name = name.clone();
            let is_system = matches!(name.as_str(), "postgres" | "template0" | "template1");
            table = table.child(
                h_flex()
                    .id(SharedString::from(row.clone()))
                    .h(px(42.))
                    .px(px(14.))
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.table_row_border)
                    .child(
                        Label::new(name.clone())
                            .id(SharedString::from(row_part_id(
                                "db-row", &view.id, &name, "name",
                            )))
                            .flex_1()
                            .font_family(mono.clone()),
                    )
                    .child(
                        Label::new(db.owner().to_owned())
                            .id(SharedString::from(row_part_id(
                                "db-row", &view.id, &name, "owner",
                            )))
                            .w(px(140.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(format::format_bytes(
                            u64::try_from(db.size_bytes()).unwrap_or(0),
                        ))
                        .id(SharedString::from(row_part_id(
                            "db-row", &view.id, &name, "size",
                        )))
                        .w(px(100.)),
                    )
                    .child(
                        h_flex().w(px(80.)).justify_end().child(
                            Button::new(SharedString::from(row_part_id(
                                "db-row", &view.id, &name, "delete",
                            )))
                            .ghost()
                            .small()
                            .compact()
                            .icon(gpui_kit::assets::IconName::Delete)
                            .tooltip(strings::UI_DATABASES_DELETE)
                            .accessibility_label(strings::UI_DATABASES_DELETE)
                            .disabled(is_system)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.open_drop_dialog(
                                        &id_delete,
                                        id_del_name.clone(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        ),
                    ),
            );
            let _ = (id_open, id_name);
        }

        table
    }

    /// The User & hak akses card: role list and the create-user form.
    fn render_users_card(
        &self,
        view: &InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let admin = self.admin_state_for(&view.id);
        let users = admin
            .map(|admin| admin.users().to_vec())
            .unwrap_or_default();
        let loading = admin.is_none_or(|admin| admin.pending());
        let busy = admin.is_some_and(|admin| admin.is_busy(AdminAction::CreateUser));
        let mono = theme.mono_font_family.clone();
        let databases: Vec<String> = admin
            .map(|admin| admin.rows().iter().map(|db| db.name().to_owned()).collect())
            .unwrap_or_default();

        let mut list = v_flex().gap_1();
        if loading {
            list = list.child(
                Label::new(strings::UI_DATABASES_USERS_LOADING)
                    .id(SharedString::from(row_part_id(
                        "users-loading",
                        &view.id,
                        "row",
                        "label",
                    )))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        } else if users.is_empty() {
            list = list.child(
                Label::new(strings::UI_DATABASES_USERS_EMPTY)
                    .id(SharedString::from(row_part_id(
                        "users-empty",
                        &view.id,
                        "row",
                        "label",
                    )))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }
        for user in &users {
            let name = user.name().to_owned();
            list = list.child(
                h_flex()
                    .id(SharedString::from(row_id("user-row", &view.id, &name)))
                    .gap_3()
                    .h(px(34.))
                    .child(
                        Label::new(name.clone())
                            .id(SharedString::from(row_part_id(
                                "user-row", &view.id, &name, "name",
                            )))
                            .flex_1()
                            .font_family(mono.clone()),
                    )
                    .child(
                        Label::new(user_label(user).to_owned())
                            .id(SharedString::from(row_part_id(
                                "user-row", &view.id, &name, "level",
                            )))
                            .px_2()
                            .rounded_full()
                            .bg(theme.muted)
                            .text_xs(),
                    ),
            );
        }

        let level = self.user_level();
        let selected_db = self.user_db_choice().map(str::to_owned);
        let has_databases = !databases.is_empty();
        let mut db_choices = h_flex()
            .gap_2()
            .flex_wrap()
            .id(SharedString::from(row_part_id(
                "users-db-choices",
                &view.id,
                "row",
                "list",
            )));
        for name in databases {
            let chosen = selected_db.as_deref() == Some(name.as_str());
            let pick = name.clone();
            db_choices = db_choices.child(
                Button::new(SharedString::from(row_part_id(
                    "users-db-choice",
                    &view.id,
                    &name,
                    "button",
                )))
                .label(name)
                .small()
                .compact()
                .when(chosen, |this| this.primary())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_user_database(pick.clone(), cx);
                })),
            );
        }
        if !has_databases {
            db_choices = db_choices.child(
                Label::new(strings::UI_DATABASES_EMPTY)
                    .id(SharedString::from(row_part_id(
                        "users-db-none",
                        &view.id,
                        "row",
                        "label",
                    )))
                    .text_xs()
                    .text_color(theme.muted_foreground),
            );
        }

        let mut levels = h_flex().gap_2().id(SharedString::from(row_part_id(
            "users-levels",
            &view.id,
            "row",
            "list",
        )));
        for candidate in [PrivilegeLevel::ReadOnly, PrivilegeLevel::ReadWrite] {
            let label = level_label(candidate);
            let chosen = candidate == level;
            levels = levels.child(
                Button::new(SharedString::from(row_part_id(
                    "users-level",
                    &view.id,
                    label,
                    "button",
                )))
                .label(label)
                .small()
                .compact()
                .when(chosen, |this| this.primary())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.choose_user_level(candidate, cx);
                })),
            );
        }

        let id_user = view.id.clone();
        card(Some(strings::UI_DATABASES_USERS), Some("users"), theme)
            .child(
                Label::new(strings::UI_DATABASES_CREATE_USER)
                    .id(SharedString::from(row_part_id(
                        "users-create",
                        &view.id,
                        "row",
                        "title",
                    )))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(list)
            .child(section_label(strings::UI_DATABASES_USER_DATABASE))
            .child(db_choices)
            .child(section_label(strings::UI_DATABASES_USER_LEVEL))
            .child(levels)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(200.))
                            .child(Input::new(&self.user_name_input_entity()).small()),
                    )
                    .child(
                        Button::new(SharedString::from(row_part_id(
                            "users-create",
                            &view.id,
                            "row",
                            "button",
                        )))
                        .label(strings::UI_DATABASES_USER_SUBMIT)
                        .small()
                        .primary()
                        .loading(busy)
                        .disabled(busy || self.user_db_choice().is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.create_user(&id_user, cx);
                        })),
                    ),
            )
    }

    /// The Password superuser card.
    fn render_reset_card(
        &self,
        view: &InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let admin = self.admin_state_for(&view.id);
        let busy = admin.is_some_and(|admin| admin.is_busy(AdminAction::ResetPassword));
        let reset_failed = admin.is_some_and(|admin| admin.reset_failed);
        let id = view.id.clone();

        let mut body = card(
            Some(strings::UI_DATABASES_PASSWORD_HEADING),
            Some("reset"),
            theme,
        )
        .child(Label::new(strings::UI_DATABASES_PASSWORD_BODY).text_color(theme.muted_foreground))
        .child(
            Button::new(SharedString::from(row_part_id(
                "reset", &view.id, "click", "button",
            )))
            .label(strings::UI_DATABASES_RESET)
            .loading(busy)
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_reset_confirm(&id, cx);
            })),
        );
        if reset_failed {
            body = body.child(
                Label::new(strings::UI_DATABASES_RESET_FAILED_NOTE)
                    .id(SharedString::from(row_part_id(
                        "reset", &view.id, "fail", "label",
                    )))
                    .text_xs()
                    .text_color(theme.warning),
            );
        }
        body
    }

    /// The last card-level error, rendered under the tables when one is set.
    ///
    /// The typed message is exactly what the service returned (`DBL-xxxx` plus
    /// its sentence). "Reload" re-reads, so a transient failure
    /// (a server that stopped, a no-password instance) is recoverable without a
    /// restart, and a no-password or Valkey instance shows the service's own
    /// reason instead of an invented note.
    pub(crate) fn render_admin_error(
        &self,
        view: &InstanceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let admin = self.admin_state_for(&view.id)?;
        let err = admin.error.as_ref().or_else(|| admin.data.error())?;
        let retry_id = view.id.clone();
        Some(
            h_flex()
                .id(SharedString::from(row_part_id(
                    "admin-error",
                    &view.id,
                    "row",
                    "line",
                )))
                .gap_2()
                .items_start()
                .child(icon(
                    gpui_kit::assets::IconName::TriangleAlert,
                    14.0,
                    theme.danger,
                ))
                .child(
                    Label::new(format!("{} — {}", err.code, err.message))
                        .id(SharedString::from(row_part_id(
                            "admin-error",
                            &view.id,
                            "row",
                            "text",
                        )))
                        .flex_1()
                        .text_xs()
                        .text_color(theme.danger),
                )
                .child(
                    Button::new(SharedString::from(row_part_id(
                        "admin-error",
                        &view.id,
                        "row",
                        "retry",
                    )))
                    .label(strings::UI_DATABASES_RETRY)
                    .small()
                    .compact()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.retry_admin(&retry_id, cx);
                    })),
                )
                .into_any_element(),
        )
    }
}
