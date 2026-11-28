//! Catalog parsing and selection tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::platform::{Arch, Os};
use crate::toolchain::catalog::{
    ArtifactOs, Format, HashOrigin, Profile, ToolId, ToolchainCatalog, compare_versions,
    parse_catalog,
};

const FULL: &str = r#"{
  "schema": 1,
  "generated_at": "2026-11-27T12:00:00Z",
  "source_url": "https://example.test/toolchains-v1.json",
  "tools": [
    {
      "id": "php",
      "versions": [
        {
          "version": "8.4.15",
          "series": "8.4",
          "artifacts": [
            {
              "os": "linux", "arch": "x86_64", "profile": "common", "format": "tar.gz",
              "url": "https://example.test/php-common.tar.gz",
              "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "size_bytes": 100, "source": "static-php-cli", "hash_origin": "pinned",
              "extensions": ["pdo_mysql", "mbstring", "mbstring"]
            },
            {
              "os": "linux", "arch": "x86_64", "profile": "bulk", "format": "tar.gz",
              "url": "https://example.test/php-bulk.tar.gz",
              "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "size_bytes": 200, "source": "static-php-cli", "hash_origin": "pinned",
              "extensions": ["intl", "sodium"]
            },
            {
              "os": "windows", "arch": "x86_64", "profile": null, "format": "zip",
              "url": "https://example.test/php-win.zip",
              "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
              "size_bytes": 300, "source": "php.net", "hash_origin": "upstream"
            }
          ]
        }
      ]
    },
    {
      "id": "node",
      "versions": [
        {
          "version": "24.11.0", "series": "24", "lts": true,
          "artifacts": [
            {
              "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
              "url": "https://example.test/node.tar.gz",
              "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
              "size_bytes": 400, "source": "nodejs.org", "hash_origin": "upstream"
            }
          ]
        }
      ]
    },
    {
      "id": "composer",
      "versions": [
        {
          "version": "2.8.4", "series": "2.8",
          "artifacts": [
            {
              "os": "any", "arch": "any", "profile": null, "format": "phar",
              "url": "https://example.test/composer.phar",
              "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
              "size_bytes": 500, "source": "getcomposer.org", "hash_origin": "upstream"
            }
          ]
        }
      ]
    }
  ]
}"#;

#[test]
fn parses_a_full_document() {
    let (catalog, skips) = parse_catalog(FULL).expect("parse");
    assert_eq!(skips.total(), 0);
    assert_eq!(catalog.tools().len(), 3);

    let php = catalog.tool(ToolId::Php).expect("php");
    assert_eq!(php.versions().len(), 1);
    let v = &php.versions()[0];
    assert_eq!(v.version(), "8.4.15");
    assert_eq!(v.series(), "8.4");
    assert_eq!(v.artifacts().len(), 3);

    // Extensions are sorted and de-duplicated.
    let common = v
        .artifacts()
        .iter()
        .find(|a| a.profile() == Some(Profile::Common))
        .expect("common");
    assert_eq!(common.extensions(), ["mbstring", "pdo_mysql"]);
    assert_eq!(common.hash_origin(), HashOrigin::Pinned);
    assert_eq!(common.size_bytes(), 100);

    let node = catalog.tool(ToolId::Node).expect("node");
    assert_eq!(node.versions()[0].lts(), Some(true));

    let composer = catalog.tool(ToolId::Composer).expect("composer");
    let phar = &composer.versions()[0].artifacts()[0];
    assert_eq!(phar.os(), ArtifactOs::Any);
    assert_eq!(phar.format(), Format::Phar);
}

#[test]
fn unknown_tool_os_profile_and_format_are_skipped_not_fatal() {
    let text = r#"{
      "schema": 1,
      "tools": [
        { "id": "ruby", "versions": [ { "version": "3.3.0", "artifacts": [] } ] },
        {
          "id": "php",
          "versions": [
            {
              "version": "8.4.15",
              "artifacts": [
                { "os": "beos", "arch": "x86_64", "profile": null, "format": "tar.gz",
                  "url": "https://e.t/a", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                  "size_bytes": 1, "hash_origin": "upstream", "source": "x" },
                { "os": "linux", "arch": "x86_64", "profile": "weird", "format": "tar.gz",
                  "url": "https://e.t/b", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                  "size_bytes": 1, "hash_origin": "pinned", "source": "x" },
                { "os": "linux", "arch": "x86_64", "profile": "common", "format": "rar",
                  "url": "https://e.t/c", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                  "size_bytes": 1, "hash_origin": "pinned", "source": "x" },
                { "os": "linux", "arch": "arm64", "profile": "common", "format": "tar.gz",
                  "url": "https://e.t/d", "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                  "size_bytes": 1, "hash_origin": "pinned", "source": "x" },
                { "os": "linux", "arch": "x86_64", "profile": "common", "format": "tar.gz",
                  "url": "https://e.t/ok", "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                  "size_bytes": 1, "hash_origin": "pinned", "source": "x" }
              ]
            }
          ]
        }
      ]
    }"#;
    let (catalog, skips) = parse_catalog(text).expect("parse");
    assert_eq!(skips.tools, 1, "ruby is skipped");
    assert_eq!(skips.artifacts, 4, "four artifacts are dropped");
    let php = catalog.tool(ToolId::Php).expect("php kept");
    assert_eq!(php.versions()[0].artifacts().len(), 1);
    assert_eq!(php.versions()[0].artifacts()[0].url(), "https://e.t/ok");
}

#[test]
fn bad_sha_or_non_https_url_drops_only_that_artifact() {
    let text = r#"{
      "schema": 1,
      "tools": [{
        "id": "node",
        "versions": [{
          "version": "24.11.0", "series": "24",
          "artifacts": [
            { "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
              "url": "https://e.t/a", "sha256": "tooshort", "size_bytes": 1,
              "hash_origin": "upstream", "source": "x" },
            { "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
              "url": "http://e.t/b", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "size_bytes": 1, "hash_origin": "upstream", "source": "x" },
            { "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
              "url": "https://e.t/ok", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
              "size_bytes": 1, "hash_origin": "upstream", "source": "x" }
          ]
        }]
      }]
    }"#;
    let (catalog, skips) = parse_catalog(text).expect("parse");
    assert_eq!(skips.artifacts, 2);
    assert_eq!(
        catalog.tool(ToolId::Node).expect("node").versions()[0]
            .artifacts()
            .len(),
        1
    );
}

#[test]
fn wrong_schema_is_a_whole_file_error() {
    let err = parse_catalog(r#"{"schema": 2, "tools": []}"#).expect_err("schema 2");
    assert!(err.to_string().contains("schema"), "{err}");
}

#[test]
fn a_corrupt_document_is_a_clear_error() {
    let err = parse_catalog("{ not json").expect_err("json");
    assert!(err.to_string().contains("not valid JSON"), "{err}");
}

#[test]
fn version_without_usable_artifact_is_not_offered() {
    let text = r#"{
      "schema": 1,
      "tools": [{ "id": "composer", "versions": [ { "version": "2.8.4", "series": "2.8", "artifacts": [] } ] }]
    }"#;
    let (catalog, _skips) = parse_catalog(text).expect("parse");
    assert!(catalog.tool(ToolId::Composer).is_none());
}

#[test]
fn unknown_json_fields_are_ignored() {
    let text = r#"{
      "schema": 1,
      "future_field": { "a": 1 },
      "tools": [{
        "id": "node", "extra": true,
        "versions": [{
          "version": "24.11.0", "series": "24", "nonsense": [1,2,3],
          "artifacts": [{
            "os": "linux", "arch": "x86_64", "profile": null, "format": "tar.gz",
            "url": "https://e.t/node.tar.gz",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size_bytes": 1, "hash_origin": "upstream", "source": "x", "unknown": "y"
          }]
        }]
      }]
    }"#;
    let (catalog, skips) = parse_catalog(text).expect("parse");
    assert_eq!(skips.total(), 0);
    assert!(catalog.tool(ToolId::Node).is_some());
}

#[test]
fn select_artifact_prefers_exact_platform_and_matches_profile() {
    let (catalog, _) = parse_catalog(FULL).expect("parse");
    let php = &catalog.tool(ToolId::Php).expect("php").versions()[0];

    let bulk = php
        .select_artifact(Os::Linux, Arch::X86_64, Some(Profile::Bulk))
        .expect("bulk");
    assert_eq!(bulk.profile(), Some(Profile::Bulk));

    let common = php
        .select_artifact(Os::Linux, Arch::X86_64, Some(Profile::Common))
        .expect("common");
    assert_eq!(common.profile(), Some(Profile::Common));

    let windows = php
        .select_artifact(Os::Windows, Arch::X86_64, None)
        .expect("windows");
    assert_eq!(windows.os(), ArtifactOs::Windows);

    // Composer's `any` artifact is selected for both OSes.
    let composer = &catalog.tool(ToolId::Composer).expect("composer").versions()[0];
    assert!(
        composer
            .select_artifact(Os::Linux, Arch::X86_64, None)
            .is_ok()
    );
    assert!(
        composer
            .select_artifact(Os::Windows, Arch::X86_64, None)
            .is_ok()
    );
}

#[test]
fn select_artifact_reports_unavailable_platform() {
    let (catalog, _) = parse_catalog(FULL).expect("parse");
    let php = &catalog.tool(ToolId::Php).expect("php").versions()[0];
    let err = php
        .select_artifact(Os::Linux, Arch::Aarch64, Some(Profile::Common))
        .expect_err("aarch64 absent");
    assert!(err.to_string().contains("not available"), "{err}");
}

#[test]
fn versions_and_artifacts_are_sorted_deterministically() {
    let text = r#"{
      "schema": 1,
      "tools": [{
        "id": "php",
        "versions": [
          { "version": "8.3.2", "series": "8.3", "artifacts": [
            { "os": "linux", "arch": "x86_64", "profile": "bulk", "format": "tar.gz",
              "url": "https://e.t/b", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "size_bytes": 1, "hash_origin": "pinned", "source": "x" },
            { "os": "linux", "arch": "x86_64", "profile": "common", "format": "tar.gz",
              "url": "https://e.t/c", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "size_bytes": 1, "hash_origin": "pinned", "source": "x" }
          ]},
          { "version": "8.4.15", "series": "8.4", "artifacts": [
            { "os": "linux", "arch": "x86_64", "profile": "common", "format": "tar.gz",
              "url": "https://e.t/d", "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd", "size_bytes": 1, "hash_origin": "pinned", "source": "x" }
          ]}
        ]
      }]
    }"#;
    let (catalog, _) = parse_catalog(text).expect("parse");
    let php = catalog.tool(ToolId::Php).expect("php");
    let versions: Vec<&str> = php.versions().iter().map(|v| v.version()).collect();
    assert_eq!(versions, vec!["8.4.15", "8.3.2"], "newest first");
    let artifacts = php.versions()[1].artifacts();
    // The contract sorts artifacts by os, arch, then profile; `bulk` sorts before
    // `common` alphabetically.
    assert_eq!(artifacts[0].profile(), Some(Profile::Bulk));
    assert_eq!(artifacts[1].profile(), Some(Profile::Common));
}

#[test]
fn compare_versions_handles_prerelease_and_padding() {
    use std::cmp::Ordering;
    assert_eq!(compare_versions("8.4.15", "8.4.9"), Ordering::Greater);
    assert_eq!(compare_versions("8.4", "8.4.0"), Ordering::Equal);
    assert_eq!(compare_versions("8.4.0-rc1", "8.4.0"), Ordering::Less);
    assert_eq!(compare_versions("24.11.0", "22.1.0"), Ordering::Greater);
}

#[test]
fn catalog_accessors_expose_the_document() {
    let (catalog, _) = parse_catalog(FULL).expect("parse");
    assert_eq!(catalog.generated_at(), "2026-11-27T12:00:00Z");
    assert_eq!(
        catalog.source_url(),
        "https://example.test/toolchains-v1.json"
    );
    assert_eq!(catalog.versions(ToolId::Php).len(), 1);
    assert!(catalog.versions(ToolId::Composer).len() == 1);
    let empty: ToolchainCatalog = ToolchainCatalog::empty();
    assert!(empty.tools().is_empty());
}

#[test]
fn tool_id_round_trips_and_rejects_unknown() {
    for tool in ToolId::all() {
        assert_eq!(tool.as_str().parse::<ToolId>().ok(), Some(tool));
    }
    assert!("ruby".parse::<ToolId>().is_err());
    assert_eq!("nodejs".parse::<ToolId>().ok(), Some(ToolId::Node));
}

#[test]
fn expected_binaries_are_derived_and_windows_aware() {
    use crate::toolchain::catalog::executable_name;
    assert_eq!(executable_name("php", Os::Linux), "php");
    assert_eq!(executable_name("php", Os::Windows), "php.exe");
    assert_eq!(ToolId::Php.shims(), &["php"]);
    assert_eq!(ToolId::Node.shims(), &["node", "npm", "npx"]);
    assert_eq!(ToolId::Composer.shims(), &["composer"]);
    assert_eq!(
        ToolId::Php.primary_binary_paths(Os::Linux),
        vec!["php".to_owned()]
    );
    assert_eq!(
        ToolId::Php.primary_binary_paths(Os::Windows),
        vec!["php.exe".to_owned()]
    );
    assert_eq!(
        ToolId::Node.primary_binary_paths(Os::Windows),
        vec![
            "node.exe".to_owned(),
            "npm.cmd".to_owned(),
            "npx.cmd".to_owned()
        ]
    );
}
