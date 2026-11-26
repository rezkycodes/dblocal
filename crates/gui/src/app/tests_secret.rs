//! The password must never reach a `Label` before the user reveals it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use dblocal_core::engine::{Credential, uri as core_uri};
use dblocal_core::model::EngineKey;
use dblocal_core::secret::SecretString;

use crate::app::{ShellState, Tabs, UriView};
use crate::format;

const RAW: &str = "p@ss:w/rd%";

fn rows_text(rows: &[format::EnvLine]) -> String {
    rows.iter()
        .map(|row| match &row.key {
            Some(key) => format!("{key}={}", row.value),
            None => row.value.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn real_service_uri_never_reaches_the_card_before_show() {
    for (engine, superuser) in [
        (EngineKey::Postgresql, "postgres"),
        (EngineKey::Mysql, "root"),
        (EngineKey::Mariadb, "root"),
        (EngineKey::Valkey, "default"),
    ] {
        let credential = Credential::new(superuser, SecretString::new(RAW));
        let port = engine.canonical_port();
        let uri = core_uri::build_uri(engine, port, &credential);
        let dotenv = core_uri::build_dotenv(engine, port, &credential);
        let encoded = core_uri::encode_userinfo(RAW);

        // Exactly what `Workspace::open_tab` stores after `LocalService::connection_uri`.
        let mut state = ShellState::default();
        let id = dblocal_core::model::InstanceId::new();
        state.set_uri(&id, &uri);

        let view = state.uri(&id).expect("uri view");
        let card_text = view.text().to_owned();
        assert!(
            !card_text.contains(RAW) && !card_text.contains(&encoded),
            "{engine:?} card leaked the secret: {card_text}"
        );
        assert!(
            card_text.contains(format::SECRET_MASK),
            "{engine:?} card is not masked: {card_text}"
        );

        let hidden = crate::app::masked_env_rows(dotenv.expose(), false, Some(view.secret()));
        let hidden_text = rows_text(&hidden);
        assert!(
            !hidden_text.contains(RAW) && !hidden_text.contains(&encoded),
            "{engine:?} hidden .env leaked: {hidden_text}"
        );

        let mut revealed = UriView::masked_from(&uri);
        revealed.reveal(uri.clone());
        let shown = crate::app::masked_env_rows(dotenv.expose(), true, Some(revealed.secret()));
        let shown_text = rows_text(&shown);
        assert!(
            shown_text.contains(&encoded) || shown_text.contains(RAW),
            "{engine:?} reveal hid the secret: {shown_text}"
        );
        revealed.hide();
        assert!(!revealed.text().contains(RAW));
    }
}

#[test]
fn every_label_the_card_builds_is_masked_before_show() {
    let engine = EngineKey::Postgresql;
    let credential = Credential::new("postgres", SecretString::new(RAW));
    let uri = core_uri::build_uri(engine, engine.canonical_port(), &credential);
    let mut state = ShellState::default();
    let id = dblocal_core::model::InstanceId::new();
    state.set_uri(&id, &uri);
    let view = state.uri(&id).expect("uri view");
    let labels = [
        view.text().to_owned(),
        format::format_bytes(1234),
        format::status_label(dblocal_core::model::InstanceStatus::Running).to_owned(),
    ];
    for label in labels {
        assert!(!label.contains(RAW), "a Label leaked the secret: {label}");
    }
    let _ = Tabs::default();
}
