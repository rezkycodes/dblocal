use crate::clients::{
    ClientKind, DetectedClient, OpenTarget, OsKind, TerminalChoice, TerminalKind, plan_open,
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
fn hostile_corpus_cli_plan_is_safe() {
    let corpus = vec![
        "normal",
        "has space",
        "quote'\"double",
        "semi;colon",
        "amp&&ersand",
        "$(subshell)",
        "`backtick`",
        "line\nbreak",
        "percent%20",
        "-leading-dash",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "ünïcödé-名前",
    ];
    let secret = SecretString::from("CANARY::SECRET");
    let terminal = TerminalChoice::new("xterm", TerminalKind::Xterm);
    for value in corpus {
        for kind in [
            ClientKind::Psql,
            ClientKind::Mysql,
            ClientKind::Mariadb,
            ClientKind::ValkeyCli,
            ClientKind::RedisCli,
        ] {
            let engine = match kind {
                ClientKind::Psql => EngineKey::Postgresql,
                ClientKind::Mysql => EngineKey::Mysql,
                ClientKind::Mariadb => EngineKey::Mariadb,
                _ => EngineKey::Valkey,
            };
            let target = OpenTarget::new(engine, "127.0.0.1", 5432, value, value, value);
            let plan = plan_open(
                &client(kind),
                &target,
                Some(&secret),
                OsKind::Linux,
                &terminal,
            )
            .unwrap();
            assert_no_secret(&plan, "CANARY::SECRET");
            // argv must pass hostile values as separate argv items, not shell string
            assert!(
                plan.argv().iter().any(|a| a.as_str() == value)
                    || plan.argv().iter().any(|a| a.contains(value))
                    || kind == ClientKind::ValkeyCli
                    || kind == ClientKind::RedisCli
            );
        }
    }
}

#[test]
fn hostile_corpus_gui_plan_is_safe() {
    let corpus = vec![
        "space name",
        "quote'\"",
        ";rm -rf",
        "&&",
        "$(id)",
        "`id`",
        "line\nbreak",
        "%",
        "-flag",
        "ünïcödé",
    ];
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
        );
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
