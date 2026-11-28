//! Unit tests for the pure Toolchains view-model (`toolchain_state.rs`).
//!
//! Covers version ordering, status derivation, update detection, button
//! enablement (busy, nothing active, Composer without PHP), error mapping,
//! PATH panel states, PHP settings validation, and remove confirmation text.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use dblocal_core::platform::{Arch, Os};
use dblocal_core::toolchain::catalog::{
    Artifact, ArtifactArch, ArtifactOs, CatalogStatus, Format, HashOrigin, Profile, ToolEntry,
    ToolId, ToolVersion, ToolchainCatalog,
};
use dblocal_core::toolchain::ops::InstalledListing;
use dblocal_core::toolchain::path::Finding;
use dblocal_core::toolchain::php_ini::{PhpSettings, SettingKey};
use dblocal_core::toolchain::registry::ActiveVersion;

use super::toolchain_state::{
    InstallProgress, PathHealth, PathPanelState, PhpSettingsForm, ToolchainState, VersionKey,
    VersionRowStatus,
};

fn sha() -> String {
    "a".repeat(64)
}

fn linux_php_artifact(profile: Profile, size: u64, exts: &[&str]) -> Artifact {
    Artifact::new(
        ArtifactOs::Linux,
        ArtifactArch::X86_64,
        Some(profile),
        Format::TarGz,
        "https://dl.static-php.dev/static-php-cli/php.tar.gz",
        sha(),
        size,
        "static-php-cli",
        HashOrigin::Pinned,
        exts.iter().map(|s| (*s).to_owned()).collect(),
    )
    .expect("artifact")
}

fn sample_catalog() -> ToolchainCatalog {
    ToolchainCatalog::new(
        "2026-11-27T12:00:00Z",
        "https://example.test/toolchains-v1.json",
        vec![
            ToolEntry::new(
                ToolId::Php,
                vec![
                    ToolVersion::new(
                        "8.4.15",
                        "8.4",
                        None,
                        vec![
                            linux_php_artifact(
                                Profile::Common,
                                18_000_000,
                                &["curl", "mbstring", "openssl", "pdo_mysql", "pdo_pgsql"],
                            ),
                            linux_php_artifact(
                                Profile::Bulk,
                                28_000_000,
                                &["curl", "intl", "mbstring", "opcache", "pdo_mysql", "sodium"],
                            ),
                        ],
                    ),
                    ToolVersion::new(
                        "8.0.30",
                        "8.0",
                        None,
                        vec![linux_php_artifact(
                            Profile::Common,
                            14_000_000,
                            &["curl", "mbstring", "openssl", "pdo_mysql", "pdo_sqlite"],
                        )],
                    ),
                ],
            ),
            ToolEntry::new(
                ToolId::Node,
                vec![
                    ToolVersion::new(
                        "24.11.1",
                        "24",
                        Some(true),
                        vec![
                            Artifact::new(
                                ArtifactOs::Linux,
                                ArtifactArch::X86_64,
                                None,
                                Format::TarGz,
                                "https://nodejs.org/dist/v24.11.1/node-v24.11.1-linux-x64.tar.gz",
                                sha(),
                                58_000_000,
                                "nodejs.org",
                                HashOrigin::Upstream,
                                vec![],
                            )
                            .expect("node 24"),
                        ],
                    ),
                    ToolVersion::new(
                        "22.21.0",
                        "22",
                        Some(true),
                        vec![
                            Artifact::new(
                                ArtifactOs::Linux,
                                ArtifactArch::X86_64,
                                None,
                                Format::TarGz,
                                "https://nodejs.org/dist/v22.21.0/node-v22.21.0-linux-x64.tar.gz",
                                sha(),
                                55_000_000,
                                "nodejs.org",
                                HashOrigin::Upstream,
                                vec![],
                            )
                            .expect("node 22"),
                        ],
                    ),
                ],
            ),
            ToolEntry::new(
                ToolId::Composer,
                vec![ToolVersion::new(
                    "2.11.3",
                    "2",
                    None,
                    vec![
                        Artifact::new(
                            ArtifactOs::Any,
                            ArtifactArch::Any,
                            None,
                            Format::Phar,
                            "https://getcomposer.org/download/2.11.3/composer.phar",
                            sha(),
                            3_800_000,
                            "getcomposer.org",
                            HashOrigin::Upstream,
                            vec![],
                        )
                        .expect("composer"),
                    ],
                )],
            ),
        ],
    )
}

fn state_with_catalog() -> ToolchainState {
    ToolchainState {
        os: Os::Linux,
        arch: Arch::X86_64,
        catalog: sample_catalog(),
        catalog_status: CatalogStatus::Fresh,
        ..ToolchainState::default()
    }
}

fn installed_item(
    tool: ToolId,
    version: &str,
    profile: Option<&str>,
    size_bytes: u64,
    active: bool,
) -> InstalledListing {
    InstalledListing {
        tool,
        version: version.to_owned(),
        profile: profile.map(str::to_owned),
        size_bytes,
        active,
    }
}

#[test]
fn version_rows_order_newest_first_and_derive_status_and_updates() {
    let mut state = state_with_catalog();
    // Install an older 8.4 patch (8.4.12) and make it active.
    state.installed.push(installed_item(
        ToolId::Php,
        "8.4.12",
        Some("common"),
        17_000_000,
        true,
    ));
    state.active.insert(
        ToolId::Php,
        ActiveVersion::new(ToolId::Php, "8.4.12", Some("common".to_owned())),
    );

    let rows = state.version_rows(ToolId::Php);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].key.version, "8.4.15");
    assert_eq!(rows[0].status, VersionRowStatus::Available);
    assert!(rows[0].can_install());
    assert!(!rows[0].can_activate());
    assert!(!rows[0].can_remove());
    assert!(!rows[0].missing_pdo_pgsql_note);

    assert_eq!(rows[1].key.version, "8.4.12");
    assert_eq!(rows[1].status, VersionRowStatus::Active);
    assert_eq!(rows[1].update_available.as_deref(), Some("8.4.15"));
    assert!(rows[1].can_update());
    assert!(rows[1].can_remove());
    assert!(!rows[1].can_activate());

    assert_eq!(rows[2].key.version, "8.0.30");
    assert_eq!(rows[2].status, VersionRowStatus::Available);
    assert!(
        rows[2].missing_pdo_pgsql_note,
        "PHP 8.0 common omits pdo_pgsql and must flag the note"
    );
}

#[test]
fn busy_version_disables_all_actions_and_enables_cancel_during_download() {
    let mut state = state_with_catalog();
    let key = VersionKey::new(ToolId::Php, "8.4.15", Some(Profile::Common));
    state.installs.insert(
        key.clone(),
        InstallProgress {
            bytes_done: 9_000_000,
            bytes_total: 18_000_000,
            cancelling: false,
        },
    );

    let rows = state.version_rows(ToolId::Php);
    let row = rows.iter().find(|r| r.key == key).expect("row");
    assert!(row.busy);
    assert!(!row.can_install());
    assert!(!row.can_activate());
    assert!(!row.can_remove());
    assert!(row.can_cancel());
    assert_eq!(
        row.progress.as_ref().and_then(InstallProgress::percent),
        Some(50)
    );

    // Once cancelling is requested, cancel button disables too.
    state.installs.get_mut(&key).unwrap().cancelling = true;
    let rows2 = state.version_rows(ToolId::Php);
    let row2 = rows2.iter().find(|r| r.key == key).expect("row");
    assert!(!row2.can_cancel());
    assert_eq!(row2.progress.as_ref().unwrap().label(), "Cancelling…");
}

#[test]
fn external_lock_busy_prevents_double_install() {
    let mut state = state_with_catalog();
    let key = VersionKey::new(ToolId::Node, "24.11.1", None);
    state.externally_busy.insert(key.clone());

    let rows = state.version_rows(ToolId::Node);
    let row = rows.iter().find(|r| r.key == key).expect("row");
    assert!(row.busy);
    assert!(!row.can_install());
    assert!(!row.can_cancel());
}

#[test]
fn sidebar_summaries_and_composer_php_dependency() {
    let mut state = state_with_catalog();
    assert!(state.composer_needs_php());
    assert!(!state.php_settings_enabled());

    let summaries = state.sidebar_summaries();
    assert_eq!(summaries.len(), 3);
    assert_eq!(summaries[0].hint, "Not installed");
    assert_eq!(summaries[1].hint, "Not installed");
    assert_eq!(summaries[2].hint, "Not installed");

    // Installed but not active -> "Inactive"
    state.installed.push(installed_item(
        ToolId::Node,
        "24.11.1",
        None,
        58_000_000,
        false,
    ));
    assert_eq!(state.sidebar_summaries()[1].hint, "Inactive");

    // Active PHP bulk -> shows version and profile
    state.active.insert(
        ToolId::Php,
        ActiveVersion::new(ToolId::Php, "8.4.15", Some("bulk".to_owned())),
    );
    assert!(!state.composer_needs_php());
    assert!(state.php_settings_enabled());
    assert_eq!(state.sidebar_summaries()[0].hint, "8.4.15 · bulk");
}

#[test]
fn path_panel_classifies_three_states() {
    let shims = PathBuf::from("/home/u/.dblocal/bin");
    let block =
        "# >>> dblocal toolchains >>>\nPATH='...'\n# <<< dblocal toolchains <<<\n".to_owned();

    // 1. Not applied (ShimsDirMissing)
    let missing = PathPanelState::from_doctor(
        shims.clone(),
        false,
        &[Finding::ShimsDirMissing {
            dir: shims.display().to_string(),
        }],
        block.clone(),
        vec!["/home/u/.bashrc".to_owned()],
    );
    assert_eq!(missing.health, PathHealth::NotApplied);
    assert_eq!(missing.findings.len(), 1);
    assert!(!missing.findings[0].severe);

    // 2. Applied and first (no findings)
    let good = PathPanelState::from_doctor(
        shims.clone(),
        true,
        &[],
        block.clone(),
        vec!["/home/u/.bashrc".to_owned()],
    );
    assert_eq!(good.health, PathHealth::AppliedAndFirst);
    assert!(good.findings.is_empty());

    // 3. Shadowed by a system php
    let shadowed = PathPanelState::from_doctor(
        shims.clone(),
        true,
        &[
            Finding::ShimsDirNotFirst {
                dir: shims.display().to_string(),
                before: "/usr/bin".to_owned(),
            },
            Finding::ShadowedBinary {
                name: "php".to_owned(),
                earlier: "/usr/bin/php".to_owned(),
            },
        ],
        block,
        vec!["/home/u/.bashrc".to_owned()],
    );
    assert_eq!(shadowed.health, PathHealth::NeedsAttention);
    assert_eq!(shadowed.findings.len(), 2);
    assert!(shadowed.findings[1].severe);
    assert!(shadowed.findings[1].message.contains("/usr/bin/php"));
}

#[test]
fn php_settings_validation_and_button_enablement() {
    let mut form =
        PhpSettingsForm::from_settings(PhpSettings::default(), PathBuf::from("/tmp/php/conf.d"));
    assert!(!form.is_dirty());
    assert!(!form.can_save(true));
    assert!(!form.can_save(false));
    assert!(form.can_reset(true));
    assert!(!form.can_reset(false));

    // Invalid size produces core parser error and disables Save.
    form.set_field(SettingKey::MemoryLimit, "not-a-size".to_owned());
    assert!(form.is_dirty());
    assert!(form.errors.contains_key(&SettingKey::MemoryLimit));
    assert!(!form.can_save(true));

    // Valid size clears error and enables Save only when PHP is active.
    form.set_field(SettingKey::MemoryLimit, "1G".to_owned());
    assert!(!form.errors.contains_key(&SettingKey::MemoryLimit));
    assert!(form.can_save(true));
    assert!(!form.can_save(false));

    // Invalid timezone produces error.
    form.set_field(SettingKey::DateTimezone, "Mars/Olympus".to_owned());
    assert!(form.errors.contains_key(&SettingKey::DateTimezone));
    assert!(form.validated_updates().is_err());

    // Valid timezone passes validation.
    form.set_field(SettingKey::DateTimezone, "Asia/Jakarta".to_owned());
    assert!(form.validated_updates().is_ok());
}

#[test]
fn php_settings_validation_messages_are_complete_sentences_naming_the_value() {
    let cases: &[(SettingKey, &str)] = &[
        (SettingKey::MemoryLimit, "not-a-size"),
        (SettingKey::UploadMaxFilesize, "0"),
        (SettingKey::PostMaxSize, "12TB"),
        (SettingKey::MaxExecutionTime, "fast"),
        (SettingKey::DateTimezone, "Mars/Olympus"),
        (SettingKey::DisplayErrors, "maybe"),
    ];
    for &(key, bad_value) in cases {
        let mut form = PhpSettingsForm::from_settings(
            PhpSettings::default(),
            PathBuf::from("/tmp/php/conf.d"),
        );
        form.set_field(key, bad_value.to_owned());
        let msg = form
            .errors
            .get(&key)
            .unwrap_or_else(|| panic!("expected error for {key:?} = {bad_value:?}"));
        assert!(
            !msg.chars().next().is_some_and(|c| c.is_ascii_lowercase()),
            "{key:?}: validation message must not start with a lowercase fragment: {msg:?}"
        );
        assert!(
            msg.contains(bad_value),
            "{key:?}: validation message must include the typed value {bad_value:?}: {msg:?}"
        );
    }
    // Check the exact timezone example too.
    let mut tz_form =
        PhpSettingsForm::from_settings(PhpSettings::default(), PathBuf::from("/tmp/php/conf.d"));
    tz_form.set_field(SettingKey::DateTimezone, "Mars/Olympus".to_owned());
    assert_eq!(
        tz_form
            .errors
            .get(&SettingKey::DateTimezone)
            .map(String::as_str),
        Some("'Mars/Olympus' is not a known timezone")
    );
}

#[test]
fn remove_dialog_explains_active_and_no_auto_switch() {
    let mut state = state_with_catalog();
    state.installed.push(installed_item(
        ToolId::Php,
        "8.4.15",
        Some("common"),
        18_000_000,
        true,
    ));
    state.installed.push(installed_item(
        ToolId::Php,
        "8.0.30",
        Some("common"),
        14_000_000,
        false,
    ));
    state.active.insert(
        ToolId::Php,
        ActiveVersion::new(ToolId::Php, "8.4.15", Some("common".to_owned())),
    );

    let dlg = state
        .build_remove_dialog(ToolId::Php, "8.4.15", Some(Profile::Common))
        .expect("dialog");
    assert!(dlg.is_active);
    assert!(dlg.others_remain);
    assert!(dlg.consequence_text().contains("not auto-switch"));

    let inactive_dlg = state
        .build_remove_dialog(ToolId::Php, "8.0.30", Some(Profile::Common))
        .expect("dialog");
    assert!(!inactive_dlg.is_active);
    assert!(!inactive_dlg.consequence_text().contains("active"));
}

#[test]
fn empty_catalog_reports_unavailable_but_keeps_installed_rows() {
    let mut state = ToolchainState::default();
    assert!(state.catalog_unavailable(ToolId::Php));
    state.installed.push(installed_item(
        ToolId::Php,
        "8.4.15",
        Some("common"),
        18_000_000,
        false,
    ));
    let rows = state.version_rows(ToolId::Php);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key.version, "8.4.15");
    assert_eq!(rows[0].status, VersionRowStatus::Installed);
}
