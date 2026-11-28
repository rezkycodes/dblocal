//! Preview constructors for Toolchains headless renders and layout tests.
//!
//! All constructors seed deterministic in-memory catalogs, installed versions,
//! PATH states and PHP settings without touching `~/.dblocal`, `$HOME`, `$PATH`
//! or the network.

use std::path::PathBuf;
use std::sync::Arc;

use dblocal_core::platform::{Arch, Os};
use dblocal_core::service::DblService;
use dblocal_core::toolchain::catalog::{
    Artifact, ArtifactArch, ArtifactOs, CatalogStatus, Format, HashOrigin, Profile, ToolEntry,
    ToolId, ToolVersion, ToolchainCatalog,
};
use dblocal_core::toolchain::ops::InstalledListing;
use dblocal_core::toolchain::path::Finding;
use dblocal_core::toolchain::php_ini::{PhpSettings, SettingKey};
use dblocal_core::toolchain::registry::ActiveVersion;
use gpui_kit::{Context, Window};

use super::Workspace;
use super::toolchain_state::{
    InstallProgress, PathPanelState, PhpSettingsForm, ToolchainState, VersionKey,
};

fn sha() -> String {
    "a".repeat(64)
}

fn php_artifact(profile: Profile, size: u64, exts: &[&str]) -> Option<Artifact> {
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
    .ok()
}

fn demo_toolchain_catalog() -> ToolchainCatalog {
    ToolchainCatalog::new(
        "2026-11-27T15:51:22Z",
        "https://example.test/toolchains-v1.json",
        vec![
            ToolEntry::new(
                ToolId::Php,
                vec![
                    ToolVersion::new(
                        "8.4.15",
                        "8.4",
                        None,
                        [
                            php_artifact(
                                Profile::Common,
                                18_400_000,
                                &[
                                    "ctype",
                                    "curl",
                                    "fileinfo",
                                    "filter",
                                    "iconv",
                                    "mbstring",
                                    "openssl",
                                    "pdo_mysql",
                                    "pdo_pgsql",
                                    "pdo_sqlite",
                                    "phar",
                                    "tokenizer",
                                    "zlib",
                                ],
                            ),
                            php_artifact(
                                Profile::Bulk,
                                28_900_000,
                                &[
                                    "ctype",
                                    "curl",
                                    "intl",
                                    "mbstring",
                                    "mysqli",
                                    "opcache",
                                    "openssl",
                                    "pdo_mysql",
                                    "sodium",
                                    "zlib",
                                ],
                            ),
                        ]
                        .into_iter()
                        .flatten()
                        .collect(),
                    ),
                    ToolVersion::new(
                        "8.3.28",
                        "8.3",
                        None,
                        php_artifact(
                            Profile::Common,
                            17_200_000,
                            &[
                                "curl",
                                "mbstring",
                                "openssl",
                                "pdo_mysql",
                                "pdo_pgsql",
                                "pdo_sqlite",
                            ],
                        )
                        .into_iter()
                        .collect(),
                    ),
                    ToolVersion::new(
                        "8.2.30",
                        "8.2",
                        None,
                        php_artifact(
                            Profile::Common,
                            16_800_000,
                            &["curl", "mbstring", "openssl", "pdo_mysql", "pdo_pgsql"],
                        )
                        .into_iter()
                        .collect(),
                    ),
                    ToolVersion::new(
                        "8.0.30",
                        "8.0",
                        None,
                        php_artifact(
                            Profile::Common,
                            14_500_000,
                            &["curl", "mbstring", "openssl", "pdo_mysql", "pdo_sqlite"],
                        )
                        .into_iter()
                        .collect(),
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
                        Artifact::new(
                            ArtifactOs::Linux,
                            ArtifactArch::X86_64,
                            None,
                            Format::TarGz,
                            "https://nodejs.org/dist/v24.11.1/node-v24.11.1-linux-x64.tar.gz",
                            sha(),
                            58_330_843,
                            "nodejs.org",
                            HashOrigin::Upstream,
                            vec![],
                        )
                        .ok()
                        .into_iter()
                        .collect(),
                    ),
                    ToolVersion::new(
                        "22.23.5",
                        "22",
                        Some(true),
                        Artifact::new(
                            ArtifactOs::Linux,
                            ArtifactArch::X86_64,
                            None,
                            Format::TarGz,
                            "https://nodejs.org/dist/v22.23.5/node-v22.23.5-linux-x64.tar.gz",
                            sha(),
                            56_856_114,
                            "nodejs.org",
                            HashOrigin::Upstream,
                            vec![],
                        )
                        .ok()
                        .into_iter()
                        .collect(),
                    ),
                ],
            ),
            ToolEntry::new(
                ToolId::Composer,
                vec![ToolVersion::new(
                    "2.11.3",
                    "2",
                    None,
                    Artifact::new(
                        ArtifactOs::Any,
                        ArtifactArch::Any,
                        None,
                        Format::Phar,
                        "https://getcomposer.org/download/2.11.3/composer.phar",
                        sha(),
                        3_820_169,
                        "getcomposer.org",
                        HashOrigin::Upstream,
                        vec![],
                    )
                    .ok()
                    .into_iter()
                    .collect(),
                )],
            ),
        ],
    )
}

fn demo_block_preview() -> String {
    "# >>> dblocal toolchains >>>\n\
     # Managed by DBLocal. Remove with `dblocal toolchain path --remove`.\n\
     case \":${PATH}:\" in\n  *:'/home/demo/.dblocal/bin':*) ;;\n  \
     *) PATH='/home/demo/.dblocal/bin'${PATH:+:$PATH}; export PATH ;;\n\
     esac\n\
     # <<< dblocal toolchains <<<\n"
        .to_owned()
}

fn base_toolchain_state() -> ToolchainState {
    let mut state = ToolchainState {
        os: Os::Linux,
        arch: Arch::X86_64,
        catalog: demo_toolchain_catalog(),
        catalog_status: CatalogStatus::Fresh,
        php_profile: Profile::Common,
        ..ToolchainState::default()
    };
    state.installed = vec![
        InstalledListing {
            tool: ToolId::Php,
            version: "8.4.15".to_owned(),
            profile: Some("common".to_owned()),
            size_bytes: 18_400_000,
            active: true,
        },
        InstalledListing {
            tool: ToolId::Php,
            version: "8.3.28".to_owned(),
            profile: Some("common".to_owned()),
            size_bytes: 17_200_000,
            active: false,
        },
        InstalledListing {
            tool: ToolId::Node,
            version: "24.11.1".to_owned(),
            profile: None,
            size_bytes: 58_330_843,
            active: true,
        },
    ];
    state.active.insert(
        ToolId::Php,
        ActiveVersion::new(ToolId::Php, "8.4.15", Some("common".to_owned())),
    );
    state.active.insert(
        ToolId::Node,
        ActiveVersion::new(ToolId::Node, "24.11.1", None),
    );
    state.path = PathPanelState::from_doctor(
        PathBuf::from("/home/demo/.dblocal/bin"),
        true,
        &[],
        demo_block_preview(),
        vec![
            "/home/demo/.profile".to_owned(),
            "/home/demo/.bashrc".to_owned(),
        ],
    );
    state.php_settings = PhpSettingsForm::from_settings(
        PhpSettings::default(),
        PathBuf::from("/home/demo/.dblocal/toolchains/php/conf.d"),
    );
    state
}

impl Workspace {
    /// Preview of the PHP toolchain page: active + installed + available +
    /// one install in progress (62%) + one inline error with Retry + PHP 8.0
    /// common `pdo_pgsql` note.
    pub fn preview_toolchain_php(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.selected_tool = Some(ToolId::Php);
        tc.installs.insert(
            VersionKey::new(ToolId::Php, "8.2.30", Some(Profile::Common)),
            InstallProgress {
                bytes_done: 10_416_000,
                bytes_total: 16_800_000,
                cancelling: false,
            },
        );
        tc.row_errors.insert(
            VersionKey::new(ToolId::Php, "8.0.30", Some(Profile::Common)),
            "DBL-3004 — Checksum verification failed for php-8.0.30-cli-linux-x86_64.tar.gz."
                .to_owned(),
        );
        ws.toolchain = tc;
        ws
    }

    /// Preview of the Node.js toolchain page.
    pub fn preview_toolchain_node(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.selected_tool = Some(ToolId::Node);
        ws.toolchain = tc;
        ws
    }

    /// Preview of the Composer page when no PHP is active ("needs an active PHP").
    pub fn preview_toolchain_composer_needs_php(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.active.remove(&ToolId::Php);
        tc.installed.retain(|i| i.tool != ToolId::Php);
        tc.selected_tool = Some(ToolId::Composer);
        ws.toolchain = tc;
        ws
    }

    /// Preview with PATH panel in "Not on PATH" state.
    pub fn preview_toolchain_path_not_applied(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.selected_tool = Some(ToolId::Node);
        tc.path = PathPanelState::from_doctor(
            PathBuf::from("/home/demo/.dblocal/bin"),
            false,
            &[Finding::ShimsDirMissing {
                dir: "/home/demo/.dblocal/bin".to_owned(),
            }],
            demo_block_preview(),
            vec![
                "/home/demo/.profile".to_owned(),
                "/home/demo/.bashrc".to_owned(),
            ],
        );
        ws.toolchain = tc;
        ws
    }

    /// Preview with PATH panel in "On PATH (first)" state.
    pub fn preview_toolchain_path_applied(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::preview_toolchain_node(service, window, cx)
    }

    /// Preview with PATH panel shadowed by `/usr/bin/php`.
    pub fn preview_toolchain_path_shadowed(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.selected_tool = Some(ToolId::Node);
        tc.path = PathPanelState::from_doctor(
            PathBuf::from("/home/demo/.dblocal/bin"),
            true,
            &[
                Finding::ShimsDirNotFirst {
                    dir: "/home/demo/.dblocal/bin".to_owned(),
                    before: "/usr/bin".to_owned(),
                },
                Finding::ShadowedBinary {
                    name: "php".to_owned(),
                    earlier: "/usr/bin/php".to_owned(),
                },
            ],
            demo_block_preview(),
            vec!["/home/demo/.bashrc".to_owned()],
        );
        ws.toolchain = tc;
        ws
    }

    /// Preview with the Add-to-PATH consent dialog open.
    pub fn preview_toolchain_path_consent(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview_toolchain_path_not_applied(service, window, cx);
        ws.toolchain.path_consent_open = true;
        ws
    }

    /// Preview of the PHP page with a PHP settings validation error.
    pub fn preview_toolchain_php_settings_error(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.selected_tool = Some(ToolId::Php);
        // Keep only 1 version row so the PHP settings card is prominently visible in the viewport.
        tc.catalog = ToolchainCatalog::new(
            "2026-11-27T15:51:22Z",
            "https://example.test/toolchains-v1.json",
            vec![ToolEntry::new(
                ToolId::Php,
                vec![ToolVersion::new(
                    "8.4.15",
                    "8.4",
                    None,
                    php_artifact(
                        Profile::Common,
                        18_400_000,
                        &["curl", "mbstring", "openssl", "pdo_mysql", "pdo_pgsql"],
                    )
                    .into_iter()
                    .collect(),
                )],
            )],
        );
        tc.installed.retain(|i| i.version == "8.4.15");
        tc.php_settings
            .set_field(SettingKey::MemoryLimit, "not-a-size".to_owned());
        tc.php_settings
            .set_field(SettingKey::DateTimezone, "Mars/Olympus".to_owned());
        ws.toolchain = tc;
        ws
    }

    /// Preview with the Remove confirmation dialog open on the active PHP version.
    pub fn preview_toolchain_remove_dialog(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview_toolchain_php(service, window, cx);
        ws.toolchain.remove_dialog =
            ws.toolchain
                .build_remove_dialog(ToolId::Php, "8.4.15", Some(Profile::Common));
        ws
    }

    /// Preview of the empty / offline catalog state ("Catalog unavailable").
    pub fn preview_toolchain_empty_catalog(
        service: Arc<dyn DblService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut ws = Self::preview(service, window, cx);
        let mut tc = base_toolchain_state();
        tc.catalog = ToolchainCatalog::empty();
        tc.catalog_status = CatalogStatus::Offline;
        tc.selected_tool = Some(ToolId::Php);
        ws.toolchain = tc;
        ws
    }
}
