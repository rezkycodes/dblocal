use crate::clients::{LaunchEnv, LaunchPlan};

pub fn assert_no_secret(plan: &LaunchPlan, secret: &str) {
    assert!(!secret.is_empty(), "secret must not be empty");
    assert!(
        !plan.program().to_string_lossy().contains(secret),
        "secret leaked in program"
    );
    for arg in plan.argv() {
        assert!(!arg.contains(secret), "secret leaked in argv: {arg:?}");
    }
    if let Some(url) = plan.url() {
        assert!(!url.contains(secret), "secret leaked in url: {url:?}");
    }
    if let Some(cmd) = plan.command_string() {
        assert!(
            !cmd.contains(secret),
            "secret leaked in command_string: {cmd:?}"
        );
    }
    for env in plan.env() {
        assert_ne!(env.name(), secret, "secret leaked in env name");
        // env value is allowed to contain the secret by design (process env),
        // but never in argv/url/command string.
        let _ = env;
    }
    // temp file contents may contain secret by design for private launcher
    // scripts; the secret must not appear elsewhere.
    if let Some(temp) = plan.temp_file() {
        let _ = temp;
    }
}

#[allow(dead_code)]
pub fn env_names(env: &[LaunchEnv]) -> Vec<&str> {
    env.iter().map(LaunchEnv::name).collect()
}
