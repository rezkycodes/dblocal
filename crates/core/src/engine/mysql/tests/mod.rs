use super::*;
use crate::engine::Credential;
use crate::model::Port;
use sha2::Digest;

mod core;
mod prepare;
mod process;
mod socket_cleanup;
mod socket_limits;
mod socket_resolve;

pub(crate) const SECRET: &str = "S3cret-Canary-Pw-MySql!9x";

pub(crate) fn init_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> InitContext {
    InitContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/opt/mysql"),
        data_dir: PathBuf::from("/data"),
        port: Port::new(port).expect("port"),
        superuser: "root".into(),
        auth,
        password: password.map(SecretString::from),
    }
}

pub(crate) fn run_ctx(port: u16, auth: AuthMode, password: Option<&str>) -> RunContext {
    RunContext {
        instance_name: "app".into(),
        runtime_dir: PathBuf::from("/opt/mysql"),
        data_dir: PathBuf::from("/data"),
        logs_dir: PathBuf::from("/logs"),
        run_dir: PathBuf::from("/run"),
        port: Port::new(port).expect("port"),
        superuser: "root".into(),
        auth,
        credential: password.map(|p| Credential::new("root", SecretString::from(p))),
    }
}

pub(crate) fn assert_no_secret_in_command(cmd: &Command, secret: &str) {
    let prog = cmd.get_program().to_string_lossy();
    assert!(!prog.contains(secret), "secret in program: {prog}");
    for arg in cmd.get_args() {
        let a = arg.to_string_lossy();
        assert!(!a.contains(secret), "secret in arg: {a}");
    }
    for (k, v) in cmd.get_envs() {
        let k = k.to_string_lossy();
        assert!(!k.contains(secret), "secret in env key: {k}");
        if k == "MYSQL_PWD" {
            continue;
        }
        if let Some(v) = v {
            let v = v.to_string_lossy();
            assert!(!v.contains(secret), "secret in env value {k}={v}");
        }
    }
}

pub(crate) fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

pub(crate) fn diag_cases() -> Vec<(&'static str, StartFailure)> {
    vec![
        (
            "2026-11-21T23:56:30.686109Z 0 [ERROR] [MY-010262] [Server] Can't start server: Bind on TCP/IP port: Address already in use\n2026-11-21T23:56:30.686124Z 0 [ERROR] [MY-010257] [Server] Do you already have another mysqld server running on port: 41102 ?\n",
            StartFailure::PortInUse,
        ),
        (
            "Can't create test file /tmp/x/ro/mysqld_tmp_file_case_insensitive_test.lower-test\nmysqld: File './binlog.index' not found (OS errno 13 - Permission denied)\n",
            StartFailure::PermissionDenied,
        ),
        // P4: four distinct cases that keep Unknown but deserve pinned fixtures.
        (
            "2026-11-21T23:56:34.517296Z 0 [ERROR] [MY-010267] [Server] The socket file path is too long (> 107): /tmp/very/long/path/mysqld.sock\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:33.765723Z 0 [ERROR] [MY-000058] [Server] Unknown suffix 'n' used for variable 'port' (value 'not-a-number').\n2026-11-21T23:56:33.765729Z 0 [ERROR] [MY-000077] [Server] /tmp/.../bin/mysqld: Error while setting value 'not-a-number' to 'port'.\n2026-11-21T23:56:33.765762Z 0 [ERROR] [MY-010119] [Server] Aborting\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:32.516055Z 1 [ERROR] [MY-012646] [InnoDB] File ./ibdata1: 'open' returned OS error 71. Cannot continue operation\n2026-11-21T23:56:32.516068Z 1 [ERROR] [MY-012981] [InnoDB] Cannot continue operation.\n",
            StartFailure::Unknown,
        ),
        (
            "2026-11-21T23:56:32.767385Z 1 [ERROR] [MY-012263] [InnoDB] The Auto-extending innodb_system data file './ibdata1' is of a different size 0 pages\n2026-11-21T23:56:33.268000Z 0 [ERROR] [MY-010020] [Server] Data Dictionary initialization failed.\n",
            StartFailure::Unknown,
        ),
        (
            "mysqld: error while loading shared libraries: libfoo.so.1: cannot open shared object file: No such file or directory\n",
            StartFailure::BinaryMissing,
        ),
        (
            "bash: /opt/mysql/bin/mysqld: No such file or directory\n",
            StartFailure::BinaryMissing,
        ),
    ]
}

pub(crate) fn write_fake_bin(dir: &Path, name: &str, script: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let p = bin.join(name);
    // Write to a temp name, chmod it, then rename into place so a concurrent
    // fork/exec in another test never sees a half-written or write-locked file
    // ("Text file busy", ETXTBSY).
    let tmp = bin.join(format!(".{name}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let _ = fs::remove_file(&p);
    fs::rename(&tmp, &p).expect("rename script");
    p
}

/// Runs `f` until it stops failing with ETXTBSY.
///
/// Another test in the same binary can fork while our fake script is still open
/// for writing; the forked child inherits that descriptor and the following exec
/// of the script fails with `Text file busy` (os error 26). That is a test
/// environment artifact, not product behaviour, so retry briefly.
pub(crate) fn retry_etxtbsy<T, E>(
    mut f: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E>
where
    E: std::fmt::Display,
{
    let mut last = None;
    for _ in 0..500 {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !err.to_string().contains("Text file busy") {
                    return Err(err);
                }
                last = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    Err(last.expect("retry budget always records the last ETXTBSY"))
}

pub(crate) fn long_run_under(dir: &Path) -> PathBuf {
    dir.join("r".repeat(140)).join("run")
}
