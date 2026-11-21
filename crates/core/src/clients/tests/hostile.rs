use crate::clients::{
    ClientError, ClientKind, DetectedClient, OpenTarget, OsKind, TerminalChoice, TerminalKind,
    plan_open,
};
use crate::model::EngineKey;
use crate::secret::SecretString;

use super::scan::assert_no_secret;

fn client(kind: ClientKind) -> DetectedClient {
    DetectedClient::new(
        kind,
        std::path::PathBuf::from("/bin/client"),
        kind.supported_engines(),
    )
}

#[test]
fn open_target_rejects_hostile_fields() {
    let bad = [
        ("host space", "postgres", "db", "name"),
        ("127.0.0.1", "-user", "db", "name"),
        ("127.0.0.1", "user;rm", "db", "name"),
        ("127.0.0.1", "postgres", "--init-command=x", "name"),
        ("127.0.0.1", "postgres", "db;rm", "name"),
        ("127.0.0.1", "postgres", "db", "bad\nname"),
        ("8.8.8.8", "postgres", "db", "name"),
    ];
    for (host, user, db, name) in bad {
        let err = OpenTarget::new(EngineKey::Postgresql, host, 5432, user, db, name).unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidTarget { .. }),
            "{host} {user} {db} {name}: {err:?}"
        );
    }
}

#[test]
fn valid_hostile_looking_but_safe_fields_do_not_leak_secret() {
    let secret = SecretString::from("CANARY::SECRET");
    let terminal = TerminalChoice::new("xterm", TerminalKind::Xterm);
    let names = ["normal", "my-app.v2", "name_1"];
    for name in names {
        let target = OpenTarget::new(
            EngineKey::Postgresql,
            "127.0.0.1",
            5432,
            "postgres",
            "postgres",
            name,
        )
        .unwrap();
        let plan = plan_open(
            &client(ClientKind::Psql),
            &target,
            Some(&secret),
            OsKind::Linux,
            &terminal,
        )
        .unwrap();
        assert_no_secret(&plan, "CANARY::SECRET");
    }
}

#[test]
fn gui_plan_is_safe_for_valid_names() {
    let corpus = ["normal", "my.app-1", "name_2"];
    let secret = SecretString::from("CANARY::SECRET");
    let terminal = TerminalChoice::new("xterm", TerminalKind::Xterm);
    for value in corpus {
        let target = OpenTarget::new(
            EngineKey::Postgresql,
            "127.0.0.1",
            5432,
            "postgres",
            "postgres",
            value,
        )
        .unwrap();
        for kind in [
            ClientKind::TablePlus,
            ClientKind::DBeaver,
            ClientKind::DbDelve,
        ] {
            let plan = plan_open(
                &client(kind),
                &target,
                Some(&secret),
                OsKind::Linux,
                &terminal,
            )
            .unwrap();
            assert_no_secret(&plan, "CANARY::SECRET");
        }
    }
}
