//! PHP settings validation, rendering and round-trip tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use tempfile::tempdir;

use crate::toolchain::fsops::ToolchainPaths;
use crate::toolchain::php_ini::{
    PhpSettings, SettingKey, SettingValue, SizeValue, write_managed_ini, write_windows_base_ini,
};

#[test]
fn defaults_render_a_full_ini() {
    let settings = PhpSettings::default();
    let ini = settings.render_ini();
    assert!(ini.contains("memory_limit = 512M"), "{ini}");
    assert!(ini.contains("upload_max_filesize = 64M"), "{ini}");
    assert!(ini.contains("post_max_size = 64M"), "{ini}");
    assert!(ini.contains("max_execution_time = 120"), "{ini}");
    assert!(ini.contains("date.timezone = UTC"), "{ini}");
    assert!(ini.contains("display_errors = On"), "{ini}");
    assert!(ini.contains("Do not edit by hand"), "{ini}");
}

#[test]
fn sizes_accept_the_documented_forms() {
    for (text, bytes) in [
        ("512M", 512 * 1024 * 1024),
        ("64m", 64 * 1024 * 1024),
        ("1G", 1024 * 1024 * 1024),
        ("1024", 1024),
        ("8K", 8 * 1024),
    ] {
        let size = SizeValue::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(size.bytes(), bytes, "{text}");
    }
    let unlimited = SizeValue::parse("-1").expect("unlimited");
    assert_eq!(unlimited.bytes(), -1);
    assert_eq!(unlimited.render(), "-1");
}

#[test]
fn sizes_keep_the_users_suffix() {
    assert_eq!(SizeValue::parse("1g").expect("g").render(), "1G");
    assert_eq!(SizeValue::parse("512m").expect("m").render(), "512M");
    assert_eq!(SizeValue::parse("64").expect("raw").render(), "64");
}

#[test]
fn hostile_values_are_rejected() {
    for bad in [
        "512M\nmemory_limit=1G",
        "512M\rmemory_limit=1G",
        "512\"M",
        "512'M",
        "512;M",
        "5 12M",
        "512X",
        "0M",
        "+512M",
        "",
    ] {
        assert!(SizeValue::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn timezone_must_be_a_real_name() {
    assert!(matches!(
        PhpSettings::parse_value(SettingKey::DateTimezone, "Europe/London").expect("valid"),
        SettingValue::Timezone(_)
    ));
    for bad in ["Not/AZone", "", "UTC\nfoo", "UTC;-1"] {
        assert!(
            PhpSettings::parse_value(SettingKey::DateTimezone, bad).is_err(),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn seconds_rejects_non_integers() {
    assert!(PhpSettings::parse_value(SettingKey::MaxExecutionTime, "120").is_ok());
    for bad in ["-1", "1.5", "abc", ""] {
        assert!(
            PhpSettings::parse_value(SettingKey::MaxExecutionTime, bad).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn display_errors_toggle_accepts_the_usual_spellings() {
    for on in ["1", "on", "true", "YES"] {
        assert_eq!(
            PhpSettings::parse_value(SettingKey::DisplayErrors, on).expect("on"),
            SettingValue::Toggle(true)
        );
    }
    for off in ["0", "off", "false", "no"] {
        assert_eq!(
            PhpSettings::parse_value(SettingKey::DisplayErrors, off).expect("off"),
            SettingValue::Toggle(false)
        );
    }
    assert!(PhpSettings::parse_value(SettingKey::DisplayErrors, "maybe").is_err());
}

#[test]
fn key_round_trips() {
    for key in SettingKey::all() {
        assert_eq!(key.as_str().parse::<SettingKey>().ok(), Some(key));
    }
    assert!("upload_max_file_size".parse::<SettingKey>().is_err());
}

#[test]
fn parse_pairs_builds_a_validated_update() {
    let pairs = vec![
        "memory_limit=1G".to_owned(),
        "display_errors=off".to_owned(),
    ];
    let parsed = PhpSettings::parse_pairs(&pairs).expect("pairs");
    assert_eq!(parsed.len(), 2);
    let mut settings = PhpSettings::default();
    for (key, value) in parsed {
        settings = settings.with_value(key, value);
    }
    let ini = settings.render_ini();
    assert!(ini.contains("memory_limit = 1G"), "{ini}");
    assert!(ini.contains("display_errors = Off"), "{ini}");

    assert!(PhpSettings::parse_pairs(&["memory_limit".to_owned()]).is_err());
    assert!(PhpSettings::parse_pairs(&["bogus=1".to_owned()]).is_err());
}

#[test]
fn parse_ini_round_trips_and_ignores_junk() {
    let settings = PhpSettings::default()
        .with_value(SettingKey::MemoryLimit, SettingValue::Size(SizeValue::parse("1G").unwrap()))
        .with_value(SettingKey::DateTimezone, SettingValue::Timezone("Asia/Jakarta".into()));
    let ini = settings.render_ini();
    let read = PhpSettings::parse_ini(&ini);
    assert_eq!(read.get(SettingKey::MemoryLimit).render(), "1G");
    assert_eq!(read.get(SettingKey::DateTimezone).render(), "Asia/Jakarta");

    // A hand-edited broken value falls back to the default, junk keys are ignored.
    let broken = "memory_limit = not a size\nunknown_setting = 1\nrandom line\n";
    let read = PhpSettings::parse_ini(broken);
    assert_eq!(
        read.get(SettingKey::MemoryLimit).render(),
        PhpSettings::default().get(SettingKey::MemoryLimit).render()
    );
}

#[test]
fn write_managed_ini_is_atomic_and_parsable() {
    let dir = tempdir().expect("tmp");
    let paths = ToolchainPaths::with_root(dir.path());
    let settings = PhpSettings::default().with_value(
        SettingKey::UploadMaxFilesize,
        SettingValue::Size(SizeValue::parse("128M").unwrap()),
    );
    let path = write_managed_ini(&paths, &settings).expect("write");
    assert_eq!(path, paths.php_managed_ini());
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(text.contains("upload_max_filesize = 128M"), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "managed ini must be private");
    }
}

#[test]
fn windows_base_ini_writes_extensions_and_extension_dir() {
    let dir = tempdir().expect("tmp");
    let extensions = vec!["curl".to_owned(), "mbstring".to_owned(), "pdo_mysql".to_owned()];
    let path = write_windows_base_ini(dir.path(), &extensions).expect("write");
    let text = std::fs::read_to_string(&path).expect("read");
    assert!(text.contains("extension=curl"), "{text}");
    assert!(text.contains("extension=mbstring"), "{text}");
    assert!(text.contains("extension_dir = "), "{text}");
    assert!(text.contains("ext"), "{text}");
}
