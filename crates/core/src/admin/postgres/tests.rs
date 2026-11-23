use super::*;

fn target(host: &str) -> AdminResult<AdminTarget> {
    AdminTarget::new(
        host,
        5432,
        "postgres",
        SecretString::from("secret-password"),
    )
}

#[test]
fn loopback_hosts_are_accepted() {
    for host in ["127.0.0.1", "::1", "localhost", "LOCALHOST"] {
        let target = target(host).expect(host);
        assert_eq!(target.host(), host);
    }
}

#[test]
fn remote_hosts_are_rejected() {
    for host in [
        "0.0.0.0",
        "192.168.1.5",
        "10.0.0.1",
        "example.com",
        "::",
        "",
    ] {
        let err = target(host).expect_err(host);
        assert!(
            matches!(
                err,
                AdminError::NonLoopbackHost | AdminError::InvalidTarget(TargetProblem::EmptyHost)
            ),
            "{host}: {err:?}"
        );
    }
}

#[test]
fn target_validation_rejects_empty_superuser_and_zero_port() {
    let err = AdminTarget::new("127.0.0.1", 5432, "", SecretString::from("x"))
        .expect_err("empty superuser");
    assert_eq!(
        err,
        AdminError::InvalidTarget(TargetProblem::EmptySuperuser)
    );

    let err = AdminTarget::new("127.0.0.1", 0, "postgres", SecretString::from("x"))
        .expect_err("zero port");
    assert_eq!(err, AdminError::InvalidTarget(TargetProblem::ZeroPort));
}

#[test]
fn dial_host_pins_localhost_to_loopback() {
    assert_eq!(target("localhost").expect("valid").dial_host(), "127.0.0.1");
    assert_eq!(target("127.0.0.1").expect("valid").dial_host(), "127.0.0.1");
    assert_eq!(target("::1").expect("valid").dial_host(), "::1");
}

#[test]
fn config_sets_timeouts_and_never_debug_leaks_the_password() {
    let target = target("127.0.0.1")
        .expect("valid")
        .with_timeouts(Duration::from_secs(2), Duration::from_secs(7));
    assert_eq!(target.connect_timeout(), Duration::from_secs(2));
    assert_eq!(target.statement_timeout(), Duration::from_secs(7));
    let debug = format!("{target:?}");
    assert!(!debug.contains("secret-password"));
    assert!(debug.contains("[REDACTED]"));
    let cfg = target.config("postgres");
    assert_eq!(cfg.get_dbname(), Some("postgres"));
    assert_eq!(cfg.get_ports(), &[5432]);
}

#[test]
fn connect_errors_distinguish_auth_from_unreachable() {
    // We cannot construct tokio_postgres::Error directly; classify via SQLSTATE
    // strings through the public-facing logic by exercising classify_sql_state
    // and the connect mapping through the documented codes.
    // map_connect_error is exercised in e2e; here we lock the AuthFailed variant
    // shape and that it differs from Connect.
    let auth = AdminError::AuthFailed;
    assert_eq!(auth.code(), "DBL-8116");
    assert_ne!(auth.user_message(), AdminError::Connect.user_message());
    assert!(!auth.user_message().is_empty());
}

#[test]
fn scram_credential_does_not_contain_cleartext() {
    let password = SecretString::from("s3cret-canary-password");
    let credential = scram_credential(&password);
    assert!(credential.starts_with("SCRAM-SHA-256$4096:"));
    assert!(!credential.contains("s3cret-canary-password"));
}

#[test]
fn with_client_works_inside_current_thread_runtime() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let port = 20001u16; // closed port; we only care it doesn't panic and maps to Connect
    let target =
        AdminTarget::new("127.0.0.1", port, "postgres", SecretString::from("pw")).expect("target");
    let admin = PgAdmin::new(target);
    let err = rt.block_on(async { admin.list_databases().unwrap_err() });
    assert_eq!(err, AdminError::Connect);
}

#[test]
fn with_client_works_inside_multi_thread_runtime() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("rt");
    let port = 20002u16;
    let target =
        AdminTarget::new("127.0.0.1", port, "postgres", SecretString::from("pw")).expect("target");
    let admin = PgAdmin::new(target);
    let err = rt.block_on(async { admin.list_databases().unwrap_err() });
    assert_eq!(err, AdminError::Connect);
}

#[test]
fn with_client_works_inside_spawn_blocking() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("rt");
    let port = 20003u16;
    let target =
        AdminTarget::new("127.0.0.1", port, "postgres", SecretString::from("pw")).expect("target");
    let admin = PgAdmin::new(target);
    let err = rt.block_on(async {
        tokio::task::spawn_blocking(move || admin.list_databases().unwrap_err())
            .await
            .expect("join")
    });
    assert_eq!(err, AdminError::Connect);
}

#[test]
fn sql_states_map_to_typed_errors() {
    assert_eq!(classify_sql_state("3D000"), AdminError::DatabaseNotFound);
    assert_eq!(classify_sql_state("42P04"), AdminError::DatabaseExists);
    assert_eq!(classify_sql_state("55006"), AdminError::DatabaseInUse);
    assert_eq!(classify_sql_state("42710"), AdminError::UserExists);
    // 42704 is mapped to UserNotFound only at role call sites (F4), not globally.
    assert_eq!(
        classify_sql_state("42704"),
        AdminError::QueryFailed(SqlStateCode::parse("42704"))
    );
    assert_eq!(
        classify_sql_state("42501"),
        AdminError::InsufficientPrivilege
    );
    assert_eq!(
        classify_sql_state("XX000"),
        AdminError::QueryFailed(SqlStateCode::parse("XX000"))
    );
    // A malformed code degrades to the payload-free variant.
    assert_eq!(classify_sql_state("bad!"), AdminError::QueryFailed(None));
}
