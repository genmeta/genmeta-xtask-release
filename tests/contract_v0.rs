use sha2::Digest;

use genmeta_xtask_release::{
    contract::{ReleaseContract, load_release_contract},
    system::PackageSystem,
};

const GATEWAY_CONTRACT: &str = r#"
[package.pishoo]
manifest = "pishoo/Cargo.toml"


[package.pishoo.build.env.DHTTP_ROOT_CA]
env = "DHTTP_ROOT_CA"

[package.pishoo.build.env.DHTTP_GLOBAL_HOME]
env = "DHTTP_GLOBAL_HOME"
optional = true

[package.pishoo.brew.build.target.aarch64-apple-darwin.env.DHTTP_GLOBAL_HOME]
value = "/opt/homebrew/etc/dhttp"

[package.pishoo.deb]
revision = "1"
architecture = "target"

[package.pishoo.deb.build]
script = "xtask/release/deb/pishoo.sh"

[package.pishoo.deb.build.container]
dockerfile = "xtask/release/deb/Dockerfile"

[package.pishoo.deb.requires.pishoo-common.version]
">=" = { from = "dependency" }
"<=" = { from = "self" }

[package.pishoo.rpm]
release = "1"
architecture = "target"

[package.pishoo.rpm.build]
script = "xtask/release/rpm/pishoo.sh"

[package.pishoo.rpm.build.container]
dockerfile = "xtask/release/rpm/Dockerfile"

[package.pishoo.rpm.requires.pishoo-common.version]
">=" = { from = "dependency" }
"<=" = { from = "self" }

[package.pishoo.brew]
template = "xtask/templates/pishoo.rb.in"

[package.pishoo.brew.build]
script = "xtask/release/brew/pishoo.sh"

[package.pishoo-common]
version = "0.5.1"
description = "Common files for pishoo"
license = "Apache-2.0"
homepage = "https://dhttp.net"
repository = "https://github.com/genmeta/gateway"

[package.pishoo-common.deb]
revision = "1"
architecture = "all"

[package.pishoo-common.deb.build]
script = "xtask/release/deb/pishoo-common.sh"

[package.pishoo-common.rpm]
release = "1"
architecture = "noarch"

[package.pishoo-common.rpm.build]
script = "xtask/release/rpm/pishoo-common.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.brew]
prefix = "homebrew"
public_base_url = "https://download.dhttp.net/homebrew"
tap.repository = "genmeta/homebrew-genmeta"
tap.base_branch = "main"
tap.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"

[destination.s3.deb]
prefix = "ppa/genmeta"
suite = "genmeta"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"
fingerprint.env = "XTASK_RELEASE_APT_SIGNING_FINGERPRINT"

[destination.s3.deb.publish]
script = "xtask/release/publish/deb.sh"

[destination.s3.deb.publish.container]
dockerfile = "xtask/release/publish/deb/Dockerfile"

[destination.s3.rpm]
prefix = "rpm/pishoo"

[destination.s3.rpm.publish]
script = "xtask/release/publish/rpm.sh"

[destination.s3.rpm.publish.container]
dockerfile = "xtask/release/publish/rpm/Dockerfile"
"#;

#[test]
fn gateway_contract_places_common_only_under_deb_and_rpm() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let common = contract
        .package("pishoo-common")
        .expect("common package exists");
    assert!(common.branch(PackageSystem::Deb).is_some());
    assert!(common.branch(PackageSystem::Rpm).is_some());
    assert!(common.branch(PackageSystem::Brew).is_none());

    let pishoo = contract.package("pishoo").expect("pishoo package exists");
    assert!(
        pishoo
            .branch(PackageSystem::Brew)
            .unwrap()
            .requires()
            .is_empty()
    );
}

#[test]
fn value_build_env_binding_must_not_be_optional() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.build.env.DHTTP_GLOBAL_HOME]
value = "/opt/sample"
optional = true

[package.sample.brew]
template = "xtask/templates/sample.rb.in"

[package.sample.brew.build]
script = "xtask/release/brew/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("value-backed optional binding should fail");

    assert_eq!(
        error.to_string(),
        "package sample env binding DHTTP_GLOBAL_HOME optional requires env"
    );
}

#[test]
fn destination_env_ref_must_not_be_empty() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.brew]
template = "xtask/templates/sample.rb.in"

[package.sample.brew.build]
script = "xtask/release/brew/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = ""
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("empty destination env ref should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 endpoint env ref must not be empty"
    );
}

use genmeta_xtask_release::{
    plan::{BuildSelectionRequest, select_build_branches},
    system::RequestedTarget,
};

#[test]
fn common_target_selects_architecture_class_not_package_name() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let deb = select_build_branches(
        &contract,
        BuildSelectionRequest {
            system: PackageSystem::Deb,
            targets: vec![RequestedTarget::Common],
            features: Vec::new(),
        },
    )
    .expect("deb common selection should resolve");
    assert_eq!(
        deb.iter()
            .map(|branch| branch.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pishoo-common"]
    );

    let rpm = select_build_branches(
        &contract,
        BuildSelectionRequest {
            system: PackageSystem::Rpm,
            targets: vec![RequestedTarget::Common],
            features: Vec::new(),
        },
    )
    .expect("rpm common selection should resolve");
    assert_eq!(
        rpm.iter()
            .map(|branch| branch.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pishoo-common"]
    );
}

#[test]
fn build_selection_preserves_each_matching_target() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let selected = select_build_branches(
        &contract,
        BuildSelectionRequest {
            system: PackageSystem::Deb,
            targets: vec![
                RequestedTarget::Common,
                RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
                RequestedTarget::Triple("aarch64-unknown-linux-gnu".to_string()),
            ],
            features: Vec::new(),
        },
    )
    .expect("deb selection should resolve");

    assert_eq!(
        selected
            .iter()
            .map(|branch| (
                branch.package_id.as_str(),
                match &branch.target {
                    RequestedTarget::Common => "common",
                    RequestedTarget::Triple(target) => target.as_str(),
                }
            ))
            .collect::<Vec<_>>(),
        vec![
            ("pishoo", "x86_64-unknown-linux-gnu"),
            ("pishoo", "aarch64-unknown-linux-gnu"),
            ("pishoo-common", "common"),
        ]
    );
}

#[test]
fn requires_bounds_resolve_from_self_and_dependency() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let package_root = temp.path().join("pishoo");
    std::fs::create_dir_all(package_root.join("src")).expect("package src should create");
    std::fs::write(package_root.join("src/main.rs"), "fn main() {}")
        .expect("package main should write");
    std::fs::write(
        package_root.join("Cargo.toml"),
        r#"
[package]
name = "pishoo"
version = "0.7.0"
edition = "2024"
description = "Sample gateway package"
license = "Apache-2.0"
homepage = "https://dhttp.net"
"#,
    )
    .expect("package manifest should write");

    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let bounds = genmeta_xtask_release::requires::resolve_requires_for(
        &contract,
        temp.path(),
        "pishoo",
        PackageSystem::Deb,
    )
    .expect("requires should resolve");

    let common = bounds.get("pishoo-common").expect("common bound exists");
    assert_eq!(common.minimum.as_deref(), Some("0.5.1-1"));
    assert_eq!(common.maximum.as_deref(), Some("0.7.0-1"));
}

#[test]
fn requires_dependency_bound_uses_dependency_manifest_version() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let dependency_root = temp.path().join("sample-lib");
    std::fs::create_dir_all(dependency_root.join("src")).expect("dependency src should create");
    std::fs::write(dependency_root.join("src/lib.rs"), "").expect("dependency lib should write");
    std::fs::write(
        dependency_root.join("Cargo.toml"),
        r#"
[package]
name = "sample-lib"
version = "1.4.0"
edition = "2024"
description = "Sample library"
license = "Apache-2.0"
homepage = "https://dhttp.net"
"#,
    )
    .expect("dependency manifest should write");

    let input = r#"
[package.sample-tool]
version = "2.0.0"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-tool.deb]
revision = "1"
architecture = "target"

[package.sample-tool.deb.build]
script = "xtask/release/deb/sample-tool.sh"

[package.sample-tool.deb.requires.sample-lib.version]
">=" = { from = "dependency" }

[package.sample-lib]
manifest = "sample-lib/Cargo.toml"

[package.sample-lib.deb]
revision = "2"
architecture = "all"

[package.sample-lib.deb.build]
script = "xtask/release/deb/sample-lib.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let bounds = genmeta_xtask_release::requires::resolve_requires_for(
        &contract,
        temp.path(),
        "sample-tool",
        PackageSystem::Deb,
    )
    .expect("requires should resolve");

    let dependency = bounds
        .get("sample-lib")
        .expect("dependency bound should exist");
    assert_eq!(dependency.minimum.as_deref(), Some("1.4.0-2"));
    assert_eq!(dependency.maximum.as_deref(), None);
}

#[test]
fn requires_self_bound_uses_self_manifest_version() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let tool_root = temp.path().join("sample-tool");
    std::fs::create_dir_all(tool_root.join("src")).expect("tool src should create");
    std::fs::write(tool_root.join("src/main.rs"), "fn main() {}").expect("tool main should write");
    std::fs::write(
        tool_root.join("Cargo.toml"),
        r#"
[package]
name = "sample-tool"
version = "2.5.0"
edition = "2024"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"
"#,
    )
    .expect("tool manifest should write");

    let input = r#"
[package.sample-tool]
manifest = "sample-tool/Cargo.toml"

[package.sample-tool.rpm]
release = "3"
architecture = "target"

[package.sample-tool.rpm.build]
script = "xtask/release/rpm/sample-tool.sh"

[package.sample-tool.rpm.requires.sample-lib.version]
"<=" = { from = "self" }

[package.sample-lib]
version = "1.4.0"
description = "Sample library"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-lib.rpm]
release = "1"
architecture = "noarch"

[package.sample-lib.rpm.build]
script = "xtask/release/rpm/sample-lib.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let bounds = genmeta_xtask_release::requires::resolve_requires_for(
        &contract,
        temp.path(),
        "sample-tool",
        PackageSystem::Rpm,
    )
    .expect("requires should resolve");

    let dependency = bounds
        .get("sample-lib")
        .expect("dependency bound should exist");
    assert_eq!(dependency.minimum.as_deref(), None);
    assert_eq!(dependency.maximum.as_deref(), Some("2.5.0-3"));
}

#[test]
fn linux_requires_render_deb_bounds_as_package_relations() {
    let entries = genmeta_xtask_release::requires::linux_requirement_entries(
        PackageSystem::Deb,
        "sample-common",
        genmeta_xtask_release::requires::ResolvedVersionBounds {
            minimum: Some("0.5.1-1".to_string()),
            maximum: Some("0.7.0-1".to_string()),
        },
    )
    .expect("deb requires should render");

    assert_eq!(
        entries,
        vec![
            "sample-common (>= 0.5.1-1)".to_string(),
            "sample-common (<= 0.7.0-1)".to_string(),
        ]
    );
}

#[test]
fn linux_requires_render_rpm_bounds_as_package_relations() {
    let entries = genmeta_xtask_release::requires::linux_requirement_entries(
        PackageSystem::Rpm,
        "sample-common",
        genmeta_xtask_release::requires::ResolvedVersionBounds {
            minimum: Some("0.5.1-1".to_string()),
            maximum: Some("0.7.0-1".to_string()),
        },
    )
    .expect("rpm requires should render");

    assert_eq!(
        entries,
        vec![
            "sample-common >= 0.5.1-1".to_string(),
            "sample-common <= 0.7.0-1".to_string(),
        ]
    );
}

#[test]
fn linux_requires_reject_non_linux_package_systems() {
    let error = genmeta_xtask_release::requires::linux_requirement_entries(
        PackageSystem::Brew,
        "sample-common",
        genmeta_xtask_release::requires::ResolvedVersionBounds {
            minimum: Some("0.5.1".to_string()),
            maximum: None,
        },
    )
    .expect_err("brew requires should not render as linux dependency entries");

    assert_eq!(
        error.to_string(),
        "brew branch does not support linux dependency entries"
    );
}

#[test]
fn build_invocation_uses_script_and_container_from_contract() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::build_invocation_for(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        &["pam".to_string()],
    )
    .expect("build invocation should plan");

    assert_eq!(plan.script.to_string_lossy(), "xtask/release/deb/pishoo.sh");
    assert_eq!(
        plan.container
            .as_ref()
            .map(|container| container.image.as_str()),
        Some("xtask-release:build-pishoo-deb")
    );
}

#[test]
fn build_invocation_keeps_branch_open_when_only_some_targets_have_env_overrides() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::build_invocation_for(
        &contract,
        "pishoo",
        PackageSystem::Brew,
        RequestedTarget::Triple("x86_64-apple-darwin".to_string()),
        &[],
    )
    .expect("target without explicit env override should still plan");

    assert_eq!(
        plan.script.to_string_lossy(),
        "xtask/release/brew/pishoo.sh"
    );
    assert_eq!(
        plan.env.get("XTASK_RELEASE_TARGET").map(String::as_str),
        Some("x86_64-apple-darwin")
    );
}

#[test]
fn sibling_patch_config_is_generated_only_from_cli_patch_arguments() {
    let config = genmeta_xtask_release::sibling::render_cargo_patch_config(&[
        genmeta_xtask_release::sibling::PatchOverride {
            source: genmeta_xtask_release::sibling::PatchSource::CratesIo,
            package: "dhttp".to_string(),
            sibling: "dhttp".to_string(),
            relative_path: "dhttp".into(),
        },
        genmeta_xtask_release::sibling::PatchOverride {
            source: genmeta_xtask_release::sibling::PatchSource::Git(
                "https://github.com/genmeta/dhttp.git".to_string(),
            ),
            package: "dhttp-access".to_string(),
            sibling: "dhttp".to_string(),
            relative_path: "access".into(),
        },
    ]);

    assert!(config.contains("[patch.crates-io]"));
    assert!(config.contains("dhttp = { path = \"/sources/dhttp/dhttp\" }"));
    assert!(config.contains("[patch.\"https://github.com/genmeta/dhttp.git\"]"));
    assert!(config.contains("dhttp-access = { path = \"/sources/dhttp/access\" }"));
    assert!(!config.contains("h3x"));
    assert!(!config.contains("rankey"));
}

#[test]
fn sibling_container_plan_mounts_cli_sources_and_writes_container_patch_config() {
    let plan = genmeta_xtask_release::sibling::container_overlay_plan(
        &[
            genmeta_xtask_release::sibling::SiblingSource {
                name: "dhttp".to_string(),
                host_path: std::path::PathBuf::from("/workspace/dhttp"),
            },
            genmeta_xtask_release::sibling::SiblingSource {
                name: "h3x".to_string(),
                host_path: std::path::PathBuf::from("/workspace/h3x"),
            },
        ],
        &[
            genmeta_xtask_release::sibling::PatchOverride {
                source: genmeta_xtask_release::sibling::PatchSource::CratesIo,
                package: "dhttp".to_string(),
                sibling: "dhttp".to_string(),
                relative_path: std::path::PathBuf::from("."),
            },
            genmeta_xtask_release::sibling::PatchOverride {
                source: genmeta_xtask_release::sibling::PatchSource::Git(
                    "https://example.invalid/h3x.git".to_string(),
                ),
                package: "h3x".to_string(),
                sibling: "h3x".to_string(),
                relative_path: std::path::PathBuf::from("h3x"),
            },
        ],
    )
    .expect("container overlay should plan");

    assert_eq!(
        plan.mounts,
        vec![
            genmeta_xtask_release::sibling::ContainerMount {
                source: std::path::PathBuf::from("/workspace/dhttp"),
                destination: std::path::PathBuf::from("/sources/dhttp"),
                read_only: true,
            },
            genmeta_xtask_release::sibling::ContainerMount {
                source: std::path::PathBuf::from("/workspace/h3x"),
                destination: std::path::PathBuf::from("/sources/h3x"),
                read_only: true,
            },
        ]
    );
    assert_eq!(
        plan.cargo_config_path,
        std::path::PathBuf::from("/opt/cargo/config.toml")
    );
    assert!(
        plan.cargo_config
            .contains("dhttp = { path = \"/sources/dhttp/.\" }")
    );
    assert!(
        plan.cargo_config
            .contains("h3x = { path = \"/sources/h3x/h3x\" }")
    );
}

const GMUTILS_CONTRACT: &str = r#"
[package.gmutils]
manifest = "genmeta/Cargo.toml"

[package.gmutils.deb]
revision = "1"
architecture = "target"

[package.gmutils.deb.build]
script = "xtask/release/deb/gmutils.sh"

[package.gmutils.deb.build.container]
dockerfile = "xtask/release/deb/Dockerfile"

[package.gmutils.rpm]
release = "1"
architecture = "target"

[package.gmutils.rpm.build]
script = "xtask/release/rpm/gmutils.sh"

[package.gmutils.rpm.build.container]
dockerfile = "xtask/release/rpm/Dockerfile"

[package.gmutils.brew]
template = "xtask/templates/gmutils.rb.in"

[package.gmutils.brew.build]
script = "xtask/release/brew/gmutils.sh"

[package.gmutils.scoop]
bin = ["genmeta.exe", "genmeta-ssh.bat"]

[package.gmutils.scoop.build]
script = "xtask/release/scoop/gmutils.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.brew]
prefix = "homebrew"
public_base_url = "https://download.dhttp.net/homebrew"
tap.repository = "genmeta/homebrew-genmeta"
tap.base_branch = "main"
tap.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"

[destination.s3.scoop]
prefix = "scoop"
public_base_url = "https://download.dhttp.net/scoop"

[destination.s3.deb]
prefix = "ppa/genmeta"
suite = "genmeta"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"
fingerprint.env = "XTASK_RELEASE_APT_SIGNING_FINGERPRINT"

[destination.s3.deb.publish]
script = "xtask/release/publish/deb.sh"

[destination.s3.deb.publish.container]
dockerfile = "xtask/release/publish/deb/Dockerfile"

[destination.s3.rpm]
prefix = "rpm/gmutils"

[destination.s3.rpm.publish]
script = "xtask/release/publish/rpm.sh"

[destination.s3.rpm.publish.container]
dockerfile = "xtask/release/publish/rpm/Dockerfile"
"#;

#[test]
fn gmutils_contract_has_scoop_and_no_common_target() {
    let contract: ReleaseContract =
        toml::from_str(GMUTILS_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let gmutils = contract.package("gmutils").expect("gmutils package exists");
    assert!(gmutils.branch(PackageSystem::Scoop).is_some());

    let common = select_build_branches(
        &contract,
        BuildSelectionRequest {
            system: PackageSystem::Deb,
            targets: vec![RequestedTarget::Common],
            features: Vec::new(),
        },
    );
    assert!(common.is_err());
}

#[test]
fn old_homebrew_tables_are_rejected() {
    let input = r#"
[package.gmutils]
manifest = "genmeta/Cargo.toml"

[homebrew.template]
path = "xtask/templates/gmutils.rb.in"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let error =
        toml::from_str::<ReleaseContract>(input).expect_err("old homebrew table should fail");
    assert!(error.to_string().contains("homebrew"));
}

#[test]
fn destination_brew_table_is_rejected() {
    let input = r#"
[package.gmutils]
manifest = "genmeta/Cargo.toml"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.brew]
prefix = "homebrew"
"#;

    let error = toml::from_str::<ReleaseContract>(input).expect_err("destination.brew should fail");
    assert!(error.to_string().contains("brew"));
}

#[test]
fn package_root_version_must_not_be_package_system_version() {
    let input = r#"
[package.pishoo-common]
version = "0.5.1-1"
description = "Common files for pishoo"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.pishoo-common.deb]
revision = "1"
architecture = "all"

[package.pishoo-common.deb.build]
script = "xtask/release/deb/pishoo-common.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("root package-system version should fail");
    assert!(error.to_string().contains("source version"));
}

#[test]
fn package_system_requires_must_reference_existing_package() {
    let input = r#"
[package.sample-tool]
version = "1.0.0"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-tool.deb]
revision = "1"
architecture = "target"

[package.sample-tool.deb.requires.sample-common.version]
">=" = { from = "dependency" }

[package.sample-tool.deb.build]
script = "xtask/release/deb/sample-tool.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("missing required package should fail");
    assert_eq!(
        error.to_string(),
        "package sample-tool deb branch requires missing package sample-common"
    );
}

#[test]
fn package_system_requires_must_reference_same_system_branch() {
    let input = r#"
[package.sample-tool]
version = "1.0.0"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-tool.brew]
template = "xtask/templates/sample-tool.rb.in"

[package.sample-tool.brew.requires.sample-common.version]
">=" = { from = "dependency" }

[package.sample-tool.brew.build]
script = "xtask/release/brew/sample-tool.sh"

[package.sample-common]
version = "1.0.0"
description = "Sample common files"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-common.deb]
revision = "1"
architecture = "all"

[package.sample-common.deb.build]
script = "xtask/release/deb/sample-common.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("missing required branch should fail");
    assert_eq!(
        error.to_string(),
        "package sample-tool brew branch requires package sample-common without brew branch"
    );
}

#[test]
fn deb_destination_requires_publish_script() {
    let input = r#"
[package.sample]
version = "1.2.3"
description = "Sample"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.deb]
prefix = "ppa/sample"
suite = "sample"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"
fingerprint.env = "XTASK_RELEASE_APT_SIGNING_FINGERPRINT"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("deb destination without publish script should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 deb branch missing publish script"
    );
}

#[test]
fn rpm_destination_requires_publish_script() {
    let input = r#"
[package.sample]
version = "1.2.3"
description = "Sample"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.rpm]
release = "1"
architecture = "target"

[package.sample.rpm.build]
script = "xtask/release/rpm/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.rpm]
prefix = "rpm/sample"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("rpm destination without publish script should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 rpm branch missing publish script"
    );
}

#[test]
fn deb_destination_rejects_empty_publish_script() {
    let input = r#"
[package.sample]
version = "1.2.3"
description = "Sample"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.deb]
prefix = "ppa/sample"
suite = "sample"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"
fingerprint.env = "XTASK_RELEASE_APT_SIGNING_FINGERPRINT"

[destination.s3.deb.publish]
script = ""
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("empty deb publish script should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 deb branch missing publish script"
    );
}

#[test]
fn publish_invocation_uses_destination_system_branch() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::publish_invocation_for(&contract, PackageSystem::Rpm)
        .expect("rpm publish should plan");

    assert_eq!(
        plan.script.to_string_lossy(),
        "xtask/release/publish/rpm.sh"
    );
    assert_eq!(
        plan.container
            .as_ref()
            .map(|container| container.image.as_str()),
        Some("xtask-release:publish-rpm")
    );
}

#[test]
fn package_command_invocations_with_primary_source_overlay_only_applies_to_container_builds() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("x86_64-unknown-linux-gnu"),
            std::ffi::OsString::from("--sibling"),
            std::ffi::OsString::from("dhttp=/workspace/dhttp"),
            std::ffi::OsString::from("--patch"),
            std::ffi::OsString::from("crates-io"),
            std::ffi::OsString::from("dhttp"),
            std::ffi::OsString::from("dhttp/dhttp"),
            std::ffi::OsString::from("brew"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("aarch64-apple-darwin"),
        ],
    )
    .expect("package command should parse");
    let values = std::collections::BTreeMap::from([
        ("DHTTP_ROOT_CA".to_string(), "/tmp/root.crt".to_string()),
        ("DHTTP_STUN_SERVER".to_string(), "nat.example".to_string()),
        (
            "DHTTP_H3_DNS_SERVER".to_string(),
            "https://dns.example:4433".to_string(),
        ),
        (
            "DHTTP_HTTP_DNS_SERVER".to_string(),
            "https://dns.example".to_string(),
        ),
        ("DHTTP_MDNS_SERVICE".to_string(), "_dhttp.local".to_string()),
        (
            "DHTTP_CERT_SERVER_URL".to_string(),
            "https://license.example".to_string(),
        ),
    ]);

    let builds = genmeta_xtask_release::plan::package_command_invocations_with_primary_source(
        &contract,
        &command,
        genmeta_xtask_release::sibling::SiblingSource {
            name: "gateway".to_string(),
            host_path: std::path::PathBuf::from("/workspace/gateway"),
        },
        &values,
    )
    .expect("package command should plan");

    let deb = builds
        .iter()
        .find(|build| build.system == PackageSystem::Deb)
        .expect("deb build should be planned");
    assert_eq!(deb.package_id.as_str(), "pishoo");
    let overlay = deb
        .source_overlay
        .as_ref()
        .expect("container deb build should include source overlay");
    assert_eq!(
        overlay.mounts,
        [
            genmeta_xtask_release::sibling::ContainerMount {
                source: std::path::PathBuf::from("/workspace/gateway"),
                destination: std::path::PathBuf::from("/sources/gateway"),
                read_only: true,
            },
            genmeta_xtask_release::sibling::ContainerMount {
                source: std::path::PathBuf::from("/workspace/dhttp"),
                destination: std::path::PathBuf::from("/sources/dhttp"),
                read_only: true,
            },
        ]
    );
    assert_eq!(
        overlay.cargo_config,
        "[patch.crates-io]\ndhttp = { path = \"/sources/dhttp/dhttp\" }\n\n"
    );

    let brew = builds
        .iter()
        .find(|build| build.system == PackageSystem::Brew)
        .expect("brew build should be planned");
    assert!(brew.source_overlay.is_none());
}

#[test]
fn s3_publish_command_plan_connects_cli_systems_to_targets_and_scripts() {
    let contract: ReleaseContract =
        toml::from_str(GMUTILS_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[
            std::ffi::OsString::from("--dry-run"),
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("scoop"),
        ],
    )
    .expect("s3 publish command should parse");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "key".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_PASSPHRASE".to_string(),
            "passphrase".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_FINGERPRINT".to_string(),
            "fingerprint".to_string(),
        ),
    ]);

    let plans = genmeta_xtask_release::plan::s3_publish_command_plan(&contract, &command, &values)
        .expect("s3 publish command should plan");

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].system, PackageSystem::Deb);
    assert!(plans[0].dry_run);
    assert_eq!(
        plans[0]
            .invocation
            .as_ref()
            .map(|invocation| invocation.script.to_string_lossy().into_owned()),
        Some("xtask/release/publish/deb.sh".to_string())
    );
    match &plans[0].target {
        genmeta_xtask_release::publish::S3PublishTarget::Deb(target) => {
            assert_eq!(target.prefix.as_str(), "ppa/genmeta");
            assert_eq!(target.fingerprint, "fingerprint");
        }
        _ => panic!("expected deb publish target"),
    }

    assert_eq!(plans[1].system, PackageSystem::Scoop);
    assert!(plans[1].dry_run);
    assert!(plans[1].invocation.is_none());
    match &plans[1].target {
        genmeta_xtask_release::publish::S3PublishTarget::Scoop(target) => {
            assert_eq!(target.prefix.as_str(), "scoop");
            assert_eq!(
                target.public_base_url.as_str(),
                "https://download.dhttp.net/scoop"
            );
        }
        _ => panic!("expected scoop publish target"),
    }
}

#[test]
fn cargo_manifest_metadata_resolves_source_package_metadata() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    std::fs::create_dir(temp.path().join("src")).expect("src directory should create");
    std::fs::write(temp.path().join("src/lib.rs"), "").expect("lib target should write");
    let manifest = temp.path().join("Cargo.toml");
    std::fs::write(
        &manifest,
        r#"
[package]
name = "sample-tool"
version = "1.2.3"
edition = "2024"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"
repository = "https://github.com/genmeta/sample-tool"
"#,
    )
    .expect("manifest should write");

    let input = format!(
        r#"
[package.sample-tool]
manifest = {manifest:?}

[package.sample-tool.brew]
template = "xtask/templates/sample-tool.rb.in"

[package.sample-tool.brew.build]
script = "xtask/release/brew/sample-tool.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#,
        manifest = manifest.to_string_lossy(),
    );

    let contract: ReleaseContract = toml::from_str(&input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let metadata =
        genmeta_xtask_release::package::resolve_metadata(&contract, "sample-tool", temp.path())
            .expect("metadata should resolve");

    assert_eq!(
        metadata.source_version,
        semver::Version::parse("1.2.3").unwrap()
    );
    assert_eq!(metadata.description, "Sample tool");
    assert_eq!(metadata.license, "Apache-2.0");
    assert_eq!(metadata.homepage, "https://dhttp.net");
    assert_eq!(
        metadata.repository.as_deref(),
        Some("https://github.com/genmeta/sample-tool")
    );
}

#[test]
fn explicit_package_metadata_resolves_non_cargo_package() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let metadata = genmeta_xtask_release::package::resolve_metadata(
        &contract,
        "pishoo-common",
        std::path::Path::new("."),
    )
    .expect("metadata should resolve");

    assert_eq!(
        metadata.source_version,
        semver::Version::parse("0.5.1").unwrap()
    );
    assert_eq!(metadata.description, "Common files for pishoo");
    assert_eq!(metadata.license, "Apache-2.0");
    assert_eq!(metadata.homepage, "https://dhttp.net");
    assert_eq!(
        metadata.repository.as_deref(),
        Some("https://github.com/genmeta/gateway")
    );
}

#[test]
fn build_invocation_resolves_package_env_and_target_override() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        ("DHTTP_ROOT_CA".to_string(), "/tmp/root.crt".to_string()),
        (
            "DHTTP_GLOBAL_HOME".to_string(),
            "/runtime/should-be-overridden".to_string(),
        ),
    ]);

    let plan = genmeta_xtask_release::plan::build_invocation_with_env_values(
        &contract,
        "pishoo",
        PackageSystem::Brew,
        RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
        &[],
        &values,
    )
    .expect("build invocation should plan");

    assert_eq!(
        plan.env.get("DHTTP_ROOT_CA").map(String::as_str),
        Some("/tmp/root.crt")
    );
    assert_eq!(
        plan.env.get("DHTTP_GLOBAL_HOME").map(String::as_str),
        Some("/opt/homebrew/etc/dhttp")
    );
}

#[test]
fn optional_build_env_is_skipped_when_missing() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([(
        "DHTTP_ROOT_CA".to_string(),
        "/tmp/root.crt".to_string(),
    )]);

    let plan = genmeta_xtask_release::plan::build_invocation_with_env_values(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        &[],
        &values,
    )
    .expect("build invocation should plan");

    assert_eq!(
        plan.env.get("DHTTP_ROOT_CA").map(String::as_str),
        Some("/tmp/root.crt")
    );
    assert!(!plan.env.contains_key("DHTTP_GLOBAL_HOME"));
}

#[test]
fn optional_target_build_env_override_suppresses_package_env_value_when_missing() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.build.env.DHTTP_GLOBAL_HOME]
env = "DHTTP_GLOBAL_HOME"

[package.sample.brew]
template = "xtask/templates/sample.rb.in"

[package.sample.brew.build]
script = "xtask/release/brew/sample.sh"

[package.sample.brew.build.target.aarch64-apple-darwin.env.DHTTP_GLOBAL_HOME]
env = "TARGET_DHTTP_GLOBAL_HOME"
optional = true

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([(
        "DHTTP_GLOBAL_HOME".to_string(),
        "/package/default".to_string(),
    )]);

    let plan = genmeta_xtask_release::plan::build_invocation_with_env_values(
        &contract,
        "sample",
        PackageSystem::Brew,
        RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
        &[],
        &values,
    )
    .expect("build invocation should plan");

    assert!(!plan.env.contains_key("DHTTP_GLOBAL_HOME"));
}

#[test]
fn common_target_build_env_override_applies_to_common_target() {
    let input = r#"
[package.sample-common]
version = "1.2.3"
description = "Sample common package"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-common.build.env.CONFIG_DIR]
env = "CONFIG_DIR"

[package.sample-common.deb]
revision = "1"
architecture = "all"

[package.sample-common.deb.build]
script = "xtask/release/deb/sample-common.sh"

[package.sample-common.deb.build.target.common.env.CONFIG_DIR]
value = "/usr/share/sample"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values =
        std::collections::BTreeMap::from([("CONFIG_DIR".to_string(), "/runtime".to_string())]);

    let plan = genmeta_xtask_release::plan::build_invocation_with_env_values(
        &contract,
        "sample-common",
        PackageSystem::Deb,
        RequestedTarget::Common,
        &[],
        &values,
    )
    .expect("common build invocation should plan");
    let names = genmeta_xtask_release::plan::build_env_names(
        &contract,
        "sample-common",
        PackageSystem::Deb,
        RequestedTarget::Common,
    )
    .expect("common build env names should resolve");

    assert_eq!(
        plan.env.get("CONFIG_DIR").map(String::as_str),
        Some("/usr/share/sample")
    );
    assert!(!names.required.contains("CONFIG_DIR"));
    assert!(!names.optional.contains("CONFIG_DIR"));
}

#[test]
fn build_and_s3_publish_env_names_union_build_and_publish_refs() {
    let contract: ReleaseContract = toml::from_str(include_str!("fixtures/gateway.release.toml"))
        .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let names = genmeta_xtask_release::plan::build_and_s3_publish_env_names(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
    )
    .expect("env names should resolve");

    assert_eq!(
        names.required.into_iter().collect::<Vec<_>>(),
        vec![
            "DHTTP_CERT_SERVER_URL".to_string(),
            "DHTTP_H3_DNS_SERVER".to_string(),
            "DHTTP_HTTP_DNS_SERVER".to_string(),
            "DHTTP_MDNS_SERVICE".to_string(),
            "DHTTP_ROOT_CA".to_string(),
            "DHTTP_STUN_SERVER".to_string(),
            "XTASK_RELEASE_APT_SIGNING_FINGERPRINT".to_string(),
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "XTASK_RELEASE_APT_SIGNING_PASSPHRASE".to_string(),
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
        ]
    );
    assert_eq!(
        names.optional.into_iter().collect::<Vec<_>>(),
        vec!["DHTTP_GLOBAL_HOME".to_string()]
    );
}

#[test]
fn build_and_s3_publish_env_names_respects_target_value_overrides() {
    let contract: ReleaseContract = toml::from_str(include_str!("fixtures/gateway.release.toml"))
        .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let names = genmeta_xtask_release::plan::build_and_s3_publish_env_names(
        &contract,
        "pishoo",
        PackageSystem::Brew,
        RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
    )
    .expect("env names should resolve");

    assert!(names.required.contains("HOMEBREW_TAP_GITHUB_TOKEN"));
    assert!(names.required.contains("XTASK_RELEASE_S3_ENDPOINT_URL"));
    assert!(!names.required.contains("DHTTP_GLOBAL_HOME"));
    assert!(!names.optional.contains("DHTTP_GLOBAL_HOME"));
}

#[test]
fn build_env_names_reports_package_level_env_refs() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let names = genmeta_xtask_release::plan::build_env_names(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
    )
    .expect("build env names should resolve");

    assert!(names.required.contains("DHTTP_ROOT_CA"));
    assert!(names.optional.contains("DHTTP_GLOBAL_HOME"));
}

#[test]
fn build_env_names_applies_target_value_overrides() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let names = genmeta_xtask_release::plan::build_env_names(
        &contract,
        "pishoo",
        PackageSystem::Brew,
        RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
    )
    .expect("build env names should resolve");

    assert!(names.required.contains("DHTTP_ROOT_CA"));
    assert!(!names.optional.contains("DHTTP_GLOBAL_HOME"));
    assert!(!names.required.contains("DHTTP_GLOBAL_HOME"));
}

#[test]
fn canonical_gateway_release_toml_parses_and_validates() {
    let contract: ReleaseContract = toml::from_str(include_str!("fixtures/gateway.release.toml"))
        .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let common = contract
        .package("pishoo-common")
        .expect("common package exists");
    assert!(common.branch(PackageSystem::Deb).is_some());
    assert!(common.branch(PackageSystem::Rpm).is_some());
    assert!(common.branch(PackageSystem::Brew).is_none());
}

#[test]
fn canonical_gmutils_release_toml_parses_and_validates() {
    let contract: ReleaseContract = toml::from_str(include_str!("fixtures/gmutils.release.toml"))
        .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let gmutils = contract.package("gmutils").expect("gmutils package exists");
    assert!(gmutils.branch(PackageSystem::Deb).is_some());
    assert!(gmutils.branch(PackageSystem::Rpm).is_some());
    assert!(gmutils.branch(PackageSystem::Brew).is_some());
    assert!(gmutils.branch(PackageSystem::Scoop).is_some());
}

#[test]
fn package_name_field_is_rejected() {
    let input = r#"
[package.gmutils]
manifest = "genmeta/Cargo.toml"
name = "gmutils"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let error = toml::from_str::<ReleaseContract>(input).expect_err("package.name should fail");
    assert!(error.to_string().contains("name"));
}

#[test]
fn format_table_is_rejected() {
    let input = r#"
[format.deb]
revision = "1"

[package.gmutils]
manifest = "genmeta/Cargo.toml"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let error = toml::from_str::<ReleaseContract>(input).expect_err("format table should fail");
    assert!(error.to_string().contains("format"));
}

#[test]
fn build_container_can_be_defined_by_dockerfile() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[package.sample.deb.build.container]
dockerfile = "xtask/release/deb/Dockerfile"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::build_invocation_for(
        &contract,
        "sample",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        &[],
    )
    .expect("dockerfile container build should plan");

    let container = plan.container.expect("container should be planned");
    assert_eq!(container.image, "xtask-release:build-sample-deb");
    assert_eq!(
        container
            .build
            .expect("container image build should be planned")
            .dockerfile,
        std::path::PathBuf::from("xtask/release/deb/Dockerfile")
    );
}

#[test]
fn publish_container_can_be_defined_by_dockerfile() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.deb]
prefix = "ppa/sample"
suite = "sample"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"

[destination.s3.deb.publish]
script = "xtask/release/publish/deb.sh"

[destination.s3.deb.publish.container]
dockerfile = "xtask/release/publish/deb/Dockerfile"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::publish_invocation_for(&contract, PackageSystem::Deb)
        .expect("dockerfile container publish should plan");

    let container = plan.container.expect("container should be planned");
    assert_eq!(container.image, "xtask-release:publish-deb");
    assert_eq!(
        container
            .build
            .expect("container image build should be planned")
            .dockerfile,
        std::path::PathBuf::from("xtask/release/publish/deb/Dockerfile")
    );
}

#[test]
fn feature_script_table_is_rejected() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[package.sample.deb.build.feature.pam]
script = "xtask/release/deb/sample-pam.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let error = toml::from_str::<ReleaseContract>(input)
        .expect_err("feature script table should be rejected");
    assert!(error.to_string().contains("feature"));
}

#[test]
fn build_invocation_includes_generic_script_environment() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([(
        "DHTTP_ROOT_CA".to_string(),
        "/tmp/root.crt".to_string(),
    )]);

    let plan = genmeta_xtask_release::plan::build_invocation_with_env_values(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        &["pam".to_string()],
        &values,
    )
    .expect("build invocation should plan");

    assert_eq!(
        plan.env.get("XTASK_RELEASE_PACKAGE_ID").map(String::as_str),
        Some("pishoo")
    );
    assert_eq!(
        plan.env.get("XTASK_RELEASE_SYSTEM").map(String::as_str),
        Some("deb")
    );
    assert_eq!(
        plan.env.get("XTASK_RELEASE_TARGET").map(String::as_str),
        Some("x86_64-unknown-linux-gnu")
    );
    assert_eq!(
        plan.env.get("XTASK_RELEASE_FEATURES").map(String::as_str),
        Some("pam")
    );
}

#[test]
fn build_invocation_for_profile_sets_script_profile_environment() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let plan = genmeta_xtask_release::plan::build_invocation_for_profile(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        genmeta_xtask_release::system::BuildProfile::Debug,
        &[],
    )
    .expect("debug build invocation should plan");

    assert_eq!(
        plan.env.get("XTASK_RELEASE_PROFILE").map(String::as_str),
        Some("debug")
    );
}

#[test]
fn build_invocation_for_profile_with_env_values_preserves_profile_and_resolves_env() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([(
        "DHTTP_ROOT_CA".to_string(),
        "/tmp/root.crt".to_string(),
    )]);

    let plan = genmeta_xtask_release::plan::build_invocation_for_profile_with_env_values(
        &contract,
        "pishoo",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        genmeta_xtask_release::system::BuildProfile::Debug,
        &["pam".to_string()],
        &values,
    )
    .expect("debug build invocation with env values should plan");

    assert_eq!(
        plan.env.get("XTASK_RELEASE_PROFILE").map(String::as_str),
        Some("debug")
    );
    assert_eq!(
        plan.env.get("DHTTP_ROOT_CA").map(String::as_str),
        Some("/tmp/root.crt")
    );
    assert_eq!(
        plan.env.get("XTASK_RELEASE_FEATURES").map(String::as_str),
        Some("pam")
    );
}

#[test]
fn package_command_invocations_connect_cli_sections_to_selected_package_builds() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("common"),
            std::ffi::OsString::from("brew"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("aarch64-apple-darwin"),
            std::ffi::OsString::from("--features"),
            std::ffi::OsString::from("pam"),
            std::ffi::OsString::from("--sibling"),
            std::ffi::OsString::from("dhttp=/workspace/dhttp"),
            std::ffi::OsString::from("--patch"),
            std::ffi::OsString::from("crates-io"),
            std::ffi::OsString::from("dhttp"),
            std::ffi::OsString::from("dhttp/dhttp"),
            std::ffi::OsString::from("--debug"),
        ],
    )
    .expect("package command should parse");
    let values = std::collections::BTreeMap::from([(
        "DHTTP_ROOT_CA".to_string(),
        "/workspace/root.crt".to_string(),
    )]);

    let builds =
        genmeta_xtask_release::plan::package_command_invocations(&contract, &command, &values)
            .expect("package command should plan invocations");

    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0].package_id.as_str(), "pishoo-common");
    assert_eq!(builds[0].system, PackageSystem::Deb);
    assert_eq!(
        builds[0]
            .invocation
            .env
            .get("XTASK_RELEASE_TARGET")
            .map(String::as_str),
        Some("common")
    );
    assert_eq!(builds[1].package_id.as_str(), "pishoo");
    assert_eq!(builds[1].system, PackageSystem::Brew);
    assert_eq!(
        builds[1]
            .invocation
            .env
            .get("XTASK_RELEASE_PROFILE")
            .map(String::as_str),
        Some("debug")
    );
    assert_eq!(
        builds[1]
            .invocation
            .env
            .get("XTASK_RELEASE_FEATURES")
            .map(String::as_str),
        Some("pam")
    );
    assert_eq!(
        builds[1]
            .invocation
            .env
            .get("DHTTP_GLOBAL_HOME")
            .map(String::as_str),
        Some("/opt/homebrew/etc/dhttp")
    );
    let overlay = builds[1]
        .source_overlay
        .as_ref()
        .expect("brew build should carry section source overlay");
    assert_eq!(
        overlay.mounts,
        [genmeta_xtask_release::sibling::ContainerMount {
            source: std::path::PathBuf::from("/workspace/dhttp"),
            destination: std::path::PathBuf::from("/sources/dhttp"),
            read_only: true,
        }]
    );
    assert_eq!(
        overlay.cargo_config,
        "[patch.crates-io]\ndhttp = { path = \"/sources/dhttp/dhttp\" }\n\n"
    );
}

use genmeta_xtask_release::manifest::{
    PackageArtifact, PackageManifest, load_manifest, load_s3_publish_command_manifests,
    manifest_path, validate_manifest, verify_manifest_artifacts, write_manifest,
};

#[test]
fn manifest_path_uses_target_common_system_directory() {
    assert_eq!(
        manifest_path(std::path::Path::new("target"), PackageSystem::Deb),
        std::path::PathBuf::from("target/common/deb/manifest.toml")
    );
}

#[test]
fn load_manifest_reads_target_common_manifest_and_verifies_artifacts() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let artifact_path = target_dir.join("x86_64-unknown-linux-gnu/release/deb/sample.deb");
    std::fs::create_dir_all(artifact_path.parent().unwrap()).expect("artifact dir should create");
    std::fs::write(&artifact_path, b"package-bytes").expect("artifact should write");
    let sha256 = format!("{:x}", sha2::Sha256::digest(b"package-bytes"));

    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
            sha256,
            size: 13,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3-1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: None,
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    let path = manifest_path(&target_dir, PackageSystem::Deb);
    std::fs::create_dir_all(path.parent().unwrap()).expect("manifest dir should create");
    std::fs::write(&path, toml::to_string_pretty(&manifest).unwrap())
        .expect("manifest should write");

    let loaded = load_manifest(&target_dir, PackageSystem::Deb).expect("manifest should load");
    verify_manifest_artifacts(&target_dir, &loaded).expect("artifacts should verify");
    assert_eq!(loaded, manifest);
}

#[test]
fn load_s3_publish_command_manifests_reads_requested_manifests_and_verifies_artifacts() {
    let contract: ReleaseContract =
        toml::from_str(GMUTILS_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("brew"),
        ],
    )
    .expect("publish command should parse");
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");

    let deb_bytes = b"deb-payload";
    let deb_path = target_dir.join("x86_64-unknown-linux-gnu/release/deb/gmutils.deb");
    std::fs::create_dir_all(deb_path.parent().unwrap()).expect("deb dir should create");
    std::fs::write(&deb_path, deb_bytes).expect("deb artifact should write");
    let deb_manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "gmutils".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/gmutils.deb".to_string(),
            sha256: format!("{:x}", sha2::Sha256::digest(deb_bytes)),
            size: deb_bytes.len() as u64,
            package_name: Some("gmutils".to_string()),
            package_version: Some("0.5.1-1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: Some("gmutils.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    write_manifest(&target_dir, &deb_manifest, false).expect("deb manifest should write");

    let brew_bytes = b"brew-payload";
    let brew_path = target_dir.join("aarch64-apple-darwin/release/brew/gmutils.tar.gz");
    std::fs::create_dir_all(brew_path.parent().unwrap()).expect("brew dir should create");
    std::fs::write(&brew_path, brew_bytes).expect("brew artifact should write");
    let brew_manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "gmutils".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "aarch64-apple-darwin".to_string(),
            path: "aarch64-apple-darwin/release/brew/gmutils.tar.gz".to_string(),
            sha256: format!("{:x}", sha2::Sha256::digest(brew_bytes)),
            size: brew_bytes.len() as u64,
            package_name: None,
            package_version: None,
            architecture: None,
            archive_name: Some("gmutils.tar.gz".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    write_manifest(&target_dir, &brew_manifest, false).expect("brew manifest should write");

    let loaded = load_s3_publish_command_manifests(&target_dir, &contract, &command)
        .expect("publish manifests should load");

    assert_eq!(loaded, [deb_manifest, brew_manifest]);
}

#[test]
fn load_s3_publish_command_manifests_rejects_artifact_hash_mismatch() {
    let contract: ReleaseContract =
        toml::from_str(GMUTILS_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[std::ffi::OsString::from("brew")],
    )
    .expect("publish command should parse");
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let brew_path = target_dir.join("aarch64-apple-darwin/release/brew/gmutils.tar.gz");
    std::fs::create_dir_all(brew_path.parent().unwrap()).expect("brew dir should create");
    std::fs::write(&brew_path, b"changed").expect("brew artifact should write");
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "gmutils".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "aarch64-apple-darwin".to_string(),
            path: "aarch64-apple-darwin/release/brew/gmutils.tar.gz".to_string(),
            sha256: "0".repeat(64),
            size: 7,
            package_name: None,
            package_version: None,
            architecture: None,
            archive_name: Some("gmutils.tar.gz".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    write_manifest(&target_dir, &manifest, false).expect("brew manifest should write");

    let error = load_s3_publish_command_manifests(&target_dir, &contract, &command)
        .expect_err("publish manifest artifact mismatch should fail");

    assert_eq!(error.to_string(), "failed to verify brew package artifacts");
}

#[test]
fn verify_manifest_artifacts_rejects_hash_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let artifact_path = target_dir.join("x86_64-unknown-linux-gnu/release/deb/sample.deb");
    std::fs::create_dir_all(artifact_path.parent().unwrap()).expect("artifact dir should create");
    std::fs::write(&artifact_path, b"changed").expect("artifact should write");

    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
            sha256: "0".repeat(64),
            size: 7,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3-1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: None,
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    let error =
        verify_manifest_artifacts(&target_dir, &manifest).expect_err("hash mismatch should fail");
    assert!(error.to_string().contains("sha256 mismatch"));
}

#[test]
fn validate_manifest_rejects_linux_artifact_without_package_version() {
    let mut manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
            sha256: "0".repeat(64),
            size: 7,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3-1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: Some("sample_1.2.3-1_amd64.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    manifest.artifacts[0].package_version = None;

    let error = validate_manifest(&manifest).expect_err("missing package version should fail");

    assert_eq!(
        error.to_string(),
        "linux package artifact must include package version"
    );
}

#[test]
fn validate_manifest_rejects_archive_artifact_without_archive_name() {
    let mut manifest = writable_manifest("1.2.3");
    manifest.artifacts[0].archive_name = None;

    let error = validate_manifest(&manifest).expect_err("missing archive name should fail");

    assert_eq!(
        error.to_string(),
        "package artifact must include archive name"
    );
}

#[test]
fn validate_manifest_targets_rejects_missing_requested_brew_target() {
    let mut manifest = writable_manifest("0.5.1");
    manifest.package = "pishoo".to_string();
    manifest.artifacts[0].target = "aarch64-apple-darwin".to_string();
    manifest.artifacts[0].path =
        "aarch64-apple-darwin/release/brew/pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string();
    manifest.artifacts[0].archive_name =
        Some("pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string());

    let error = genmeta_xtask_release::manifest::validate_manifest_targets(
        &manifest,
        &[
            RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
            RequestedTarget::Triple("x86_64-apple-darwin".to_string()),
        ],
    )
    .expect_err("missing x86_64 brew target should fail");

    assert_eq!(
        error.to_string(),
        "brew package manifest is missing requested target x86_64-apple-darwin"
    );
}

#[test]
fn validate_manifest_targets_rejects_unrequested_brew_target() {
    let mut manifest = writable_manifest("0.5.1");
    manifest.package = "pishoo".to_string();
    manifest.artifacts[0].target = "aarch64-apple-darwin".to_string();
    manifest.artifacts[0].path =
        "aarch64-apple-darwin/release/brew/pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string();
    manifest.artifacts[0].archive_name =
        Some("pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string());

    let mut x86_64 = manifest.artifacts[0].clone();
    x86_64.target = "x86_64-apple-darwin".to_string();
    x86_64.path =
        "x86_64-apple-darwin/release/brew/pishoo-0.5.1-x86_64-apple-darwin.tar.gz".to_string();
    x86_64.archive_name = Some("pishoo-0.5.1-x86_64-apple-darwin.tar.gz".to_string());
    manifest.artifacts.push(x86_64);

    let mut linux = manifest.artifacts[0].clone();
    linux.target = "x86_64-unknown-linux-gnu".to_string();
    linux.path =
        "x86_64-unknown-linux-gnu/release/brew/pishoo-0.5.1-x86_64-unknown-linux-gnu.tar.gz"
            .to_string();
    linux.archive_name = Some("pishoo-0.5.1-x86_64-unknown-linux-gnu.tar.gz".to_string());
    manifest.artifacts.push(linux);

    let error = genmeta_xtask_release::manifest::validate_manifest_targets(
        &manifest,
        &[
            RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
            RequestedTarget::Triple("x86_64-apple-darwin".to_string()),
        ],
    )
    .expect_err("unrequested linux brew target should fail");

    assert_eq!(
        error.to_string(),
        "brew package manifest contains unrequested target x86_64-unknown-linux-gnu"
    );
}

#[test]
fn validate_manifest_against_contract_rejects_semver_as_deb_package_version() {
    let input = r#"
[package.sample]
version = "1.2.3"
description = "Sample"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.deb]
prefix = "ppa/sample"
suite = "sample"
signing.key.env = "XTASK_RELEASE_APT_SIGNING_KEY"
signing.passphrase.env = "XTASK_RELEASE_APT_SIGNING_PASSPHRASE"
fingerprint.env = "XTASK_RELEASE_APT_SIGNING_FINGERPRINT"

[destination.s3.deb.publish]
script = "xtask/release/publish/deb.sh"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
            sha256: "0".repeat(64),
            size: 0,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: Some("sample.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    let error =
        genmeta_xtask_release::manifest::validate_manifest_against_contract(&manifest, &contract)
            .expect_err("deb package version must include revision");

    assert_eq!(
        error.to_string(),
        "linux package artifact sample deb version 1.2.3 does not match expected 1.2.3-1"
    );
}

#[test]
fn validate_manifest_against_contract_rejects_cargo_semver_as_deb_package_version() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "pishoo".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/pishoo.deb".to_string(),
            sha256: "0".repeat(64),
            size: 0,
            package_name: Some("pishoo".to_string()),
            package_version: Some("0.5.1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: Some("pishoo.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    let error =
        genmeta_xtask_release::manifest::validate_manifest_against_contract(&manifest, &contract)
            .expect_err("cargo package deb artifact version must include revision");

    assert_eq!(
        error.to_string(),
        "linux package artifact pishoo deb version 0.5.1 does not match expected 0.5.1-1"
    );
}

#[test]
fn validate_package_command_manifest_uses_cli_requested_targets() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            std::ffi::OsString::from("brew"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("aarch64-apple-darwin"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("x86_64-apple-darwin"),
        ],
    )
    .expect("package command should parse");
    let mut manifest = writable_manifest("0.5.1");
    manifest.package = "pishoo".to_string();
    manifest.artifacts[0].target = "aarch64-apple-darwin".to_string();
    manifest.artifacts[0].path =
        "aarch64-apple-darwin/release/brew/pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string();
    manifest.artifacts[0].archive_name =
        Some("pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string());

    let error = genmeta_xtask_release::manifest::validate_package_command_manifest(
        &manifest, &contract, &command,
    )
    .expect_err("manifest missing cli-requested x86_64 target should fail");

    assert_eq!(
        error.to_string(),
        "failed to validate package manifest targets"
    );
}

#[test]
fn validate_package_command_manifest_only_uses_targets_that_select_manifest_package() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("common"),
            std::ffi::OsString::from("deb"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("x86_64-unknown-linux-gnu"),
        ],
    )
    .expect("package command should parse");
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "pishoo-common".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "common".to_string(),
            path: "common/release/deb/pishoo-common.deb".to_string(),
            sha256: "0".repeat(64),
            size: 0,
            package_name: Some("pishoo-common".to_string()),
            package_version: Some("0.5.1-1".to_string()),
            architecture: Some("all".to_string()),
            archive_name: Some("pishoo-common.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    genmeta_xtask_release::manifest::validate_package_command_manifest(
        &manifest, &contract, &command,
    )
    .expect("pishoo-common manifest should ignore targets that select pishoo");
}

#[test]
fn validate_manifest_against_contract_checks_linux_artifact_package_branches() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "pishoo".to_string(),
        version: "0.5.1".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "common".to_string(),
            path: "common/release/deb/missing-common.deb".to_string(),
            sha256: "0".repeat(64),
            size: 0,
            package_name: Some("missing-common".to_string()),
            package_version: Some("0.5.1-1".to_string()),
            architecture: Some("all".to_string()),
            archive_name: Some("missing-common.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    let error =
        genmeta_xtask_release::manifest::validate_manifest_against_contract(&manifest, &contract)
            .expect_err("unknown linux artifact package should fail");

    assert_eq!(
        error.to_string(),
        "linux package artifact missing-common does not exist"
    );
}

use genmeta_xtask_release::publish::{LinuxPackageVersion, linux_package_payloads_from_manifest};

#[test]
fn linux_payloads_are_extracted_from_package_manifest_artifacts() {
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Deb,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
            sha256: "0".repeat(64),
            size: 1,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3-1".to_string()),
            architecture: Some("amd64".to_string()),
            archive_name: Some("sample_1.2.3-1_amd64.deb".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };

    let payloads =
        linux_package_payloads_from_manifest(&manifest).expect("linux payloads should resolve");

    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0].package, "sample");
    assert_eq!(payloads[0].version, "1.2.3-1");
    assert_eq!(payloads[0].architecture, "amd64");
    assert_eq!(payloads[0].archive_name, "sample_1.2.3-1_amd64.deb");
    assert_eq!(
        payloads[0].path,
        "x86_64-unknown-linux-gnu/release/deb/sample.deb"
    );
}

#[test]
fn linux_payloads_reject_non_linux_package_manifest() {
    let mut manifest = writable_manifest("1.2.3");
    manifest.kind = PackageSystem::Brew;

    let error = linux_package_payloads_from_manifest(&manifest)
        .expect_err("brew manifest should not produce linux payloads");

    assert_eq!(
        error.to_string(),
        "brew package manifest does not contain linux package payloads"
    );
}

#[test]
fn linux_payloads_require_package_version() {
    let mut manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Rpm,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-unknown-linux-gnu".to_string(),
            path: "x86_64-unknown-linux-gnu/release/rpm/sample.rpm".to_string(),
            sha256: "0".repeat(64),
            size: 1,
            package_name: Some("sample".to_string()),
            package_version: Some("1.2.3-1".to_string()),
            architecture: Some("x86_64".to_string()),
            archive_name: Some("sample-1.2.3-1.x86_64.rpm".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    manifest.artifacts[0].package_version = None;

    let error = linux_package_payloads_from_manifest(&manifest)
        .expect_err("missing package version should fail");

    assert_eq!(
        error.to_string(),
        "linux package artifact is missing package version"
    );
}

use genmeta_xtask_release::publish::{LinuxPackagePayload, select_publishable_linux_payloads};

#[test]
fn linux_payload_key_uses_deb_pool_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");
    let payload = LinuxPackagePayload {
        package: "sample".to_string(),
        version: "1.2.3-1".to_string(),
        architecture: "amd64".to_string(),
        archive_name: "sample_1.2.3-1_amd64.deb".to_string(),
        path: "x86_64-unknown-linux-gnu/release/deb/sample.deb".to_string(),
    };

    let key =
        genmeta_xtask_release::publish::linux_payload_key(&prefix, PackageSystem::Deb, &payload)
            .expect("deb payload key should resolve");

    assert_eq!(
        key,
        "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb"
    );
}

#[test]
fn linux_payload_key_uses_rpm_package_version_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("rpm/sample")
        .expect("prefix should parse");
    let payload = LinuxPackagePayload {
        package: "sample".to_string(),
        version: "1.2.3-1".to_string(),
        architecture: "x86_64".to_string(),
        archive_name: "sample-1.2.3-1.x86_64.rpm".to_string(),
        path: "x86_64-unknown-linux-gnu/release/rpm/sample.rpm".to_string(),
    };

    let key =
        genmeta_xtask_release::publish::linux_payload_key(&prefix, PackageSystem::Rpm, &payload)
            .expect("rpm payload key should resolve");

    assert_eq!(key, "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.rpm");
}

#[test]
fn remote_deb_payload_version_from_key_uses_pool_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");

    let version = genmeta_xtask_release::publish::remote_deb_payload_version_from_key(
        &prefix,
        "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb",
    )
    .expect("remote deb payload version should parse");

    assert_eq!(version.package, "sample");
    assert_eq!(version.version, "1.2.3-1");
    assert_eq!(version.architecture, "amd64");
}

#[test]
fn remote_deb_payload_version_from_key_rejects_unexpected_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");

    let error = genmeta_xtask_release::publish::remote_deb_payload_version_from_key(
        &prefix,
        "apt/sample/sample.deb",
    )
    .expect_err("short deb key should fail");

    assert_eq!(
        error.to_string(),
        "remote deb payload key apt/sample/sample.deb has unexpected layout"
    );
}

#[test]
fn remote_deb_payload_version_from_key_requires_deb_filename() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");

    let error = genmeta_xtask_release::publish::remote_deb_payload_version_from_key(
        &prefix,
        "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.zip",
    )
    .expect_err("non-deb key should fail");

    assert_eq!(
        error.to_string(),
        "remote deb payload key apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.zip is missing deb filename"
    );
}

#[test]
fn remote_rpm_payload_version_from_key_uses_package_version_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("rpm/sample")
        .expect("prefix should parse");

    let version = genmeta_xtask_release::publish::remote_rpm_payload_version_from_key(
        &prefix,
        "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.rpm",
    )
    .expect("remote rpm payload version should parse");

    assert_eq!(version.package, "sample");
    assert_eq!(version.version, "1.2.3-1");
    assert_eq!(version.architecture, "x86_64");
}

#[test]
fn remote_rpm_payload_version_from_key_rejects_unexpected_layout() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("rpm/sample")
        .expect("prefix should parse");

    let error = genmeta_xtask_release::publish::remote_rpm_payload_version_from_key(
        &prefix,
        "rpm/sample/sample.rpm",
    )
    .expect_err("short rpm key should fail");

    assert_eq!(
        error.to_string(),
        "remote rpm payload key rpm/sample/sample.rpm has unexpected layout"
    );
}

#[test]
fn remote_rpm_payload_version_from_key_requires_rpm_extension() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("rpm/sample")
        .expect("prefix should parse");

    let error = genmeta_xtask_release::publish::remote_rpm_payload_version_from_key(
        &prefix,
        "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.zip",
    )
    .expect_err("non-rpm key should fail");

    assert_eq!(
        error.to_string(),
        "remote rpm payload key rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.zip is missing rpm filename"
    );
}

#[test]
fn remote_linux_payload_versions_from_keys_dispatches_deb_parser() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");

    let versions = genmeta_xtask_release::publish::remote_linux_payload_versions_from_keys(
        PackageSystem::Deb,
        &prefix,
        [
            "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb",
            "apt/sample/pool/main/s/sample/sample_1.2.2-1_arm64.deb",
        ],
    )
    .expect("remote deb payload versions should parse");

    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].package, "sample");
    assert_eq!(versions[0].version, "1.2.3-1");
    assert_eq!(versions[0].architecture, "amd64");
    assert_eq!(versions[1].package, "sample");
    assert_eq!(versions[1].version, "1.2.2-1");
    assert_eq!(versions[1].architecture, "arm64");
}

#[test]
fn remote_linux_payload_versions_from_keys_dispatches_rpm_parser() {
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("rpm/sample")
        .expect("prefix should parse");

    let versions = genmeta_xtask_release::publish::remote_linux_payload_versions_from_keys(
        PackageSystem::Rpm,
        &prefix,
        [
            "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.rpm",
            "rpm/sample/sample/1.2.2-1/sample-1.2.2-1.aarch64.rpm",
        ],
    )
    .expect("remote rpm payload versions should parse");

    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].package, "sample");
    assert_eq!(versions[0].version, "1.2.3-1");
    assert_eq!(versions[0].architecture, "x86_64");
    assert_eq!(versions[1].package, "sample");
    assert_eq!(versions[1].version, "1.2.2-1");
    assert_eq!(versions[1].architecture, "aarch64");
}

#[test]
fn remote_linux_payload_versions_from_keys_rejects_non_linux_system() {
    let prefix =
        genmeta_xtask_release::publish::RemotePrefix::parse("brew").expect("prefix should parse");

    let error = genmeta_xtask_release::publish::remote_linux_payload_versions_from_keys(
        PackageSystem::Brew,
        &prefix,
        ["brew/sample.rb"],
    )
    .expect_err("brew should not parse as linux payload versions");

    assert_eq!(
        error.to_string(),
        "brew package systems do not define linux payload versions"
    );
}

#[test]
fn linux_repository_upload_order_sorts_deb_payload_before_metadata_and_inrelease_last() {
    let mut keys = vec![
        "apt/sample/dists/sample/InRelease",
        "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb",
        "apt/sample/dists/sample/main/binary-amd64/Packages.gz",
        "apt/sample/dists/sample/Release.gpg",
        "apt/sample/dists/sample/Release",
    ];

    keys.sort_by_key(|key| {
        (
            genmeta_xtask_release::publish::linux_repository_upload_order(PackageSystem::Deb, key)
                .expect("deb order should resolve"),
            *key,
        )
    });

    assert_eq!(
        keys,
        vec![
            "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb",
            "apt/sample/dists/sample/main/binary-amd64/Packages.gz",
            "apt/sample/dists/sample/Release",
            "apt/sample/dists/sample/Release.gpg",
            "apt/sample/dists/sample/InRelease",
        ]
    );
}

#[test]
fn linux_repository_upload_order_sorts_rpm_payload_before_metadata_and_repomd_last() {
    let mut keys = vec![
        "rpm/sample/repodata/repomd.xml",
        "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.rpm",
        "rpm/sample/repodata/primary.xml.gz",
    ];

    keys.sort_by_key(|key| {
        (
            genmeta_xtask_release::publish::linux_repository_upload_order(PackageSystem::Rpm, key)
                .expect("rpm order should resolve"),
            *key,
        )
    });

    assert_eq!(
        keys,
        vec![
            "rpm/sample/sample/1.2.3-1/sample-1.2.3-1.x86_64.rpm",
            "rpm/sample/repodata/primary.xml.gz",
            "rpm/sample/repodata/repomd.xml",
        ]
    );
}

#[test]
fn linux_repository_upload_order_rejects_non_linux_system() {
    let error =
        genmeta_xtask_release::publish::linux_repository_upload_order(PackageSystem::Brew, "x")
            .expect_err("brew should not have linux repository upload order");

    assert_eq!(
        error.to_string(),
        "brew repositories do not define linux metadata upload order"
    );
}

#[test]
fn linux_publish_selection_skips_equal_and_older_package_arch_versions() {
    let local = vec![
        linux_payload("sample", "1.2.3-1", "amd64"),
        linux_payload("sample", "1.2.2-1", "arm64"),
    ];
    let remote = vec![
        linux_version("sample", "1.2.3-1", "amd64"),
        linux_version("sample", "1.2.3-1", "arm64"),
    ];

    let selected = select_publishable_linux_payloads(local, &remote, compare_version_strings)
        .expect("publish selection should resolve");

    assert!(selected.is_empty());
}

#[test]
fn linux_publish_selection_compares_against_latest_remote_per_package_arch() {
    let local = vec![
        linux_payload("sample", "1.2.4-1", "amd64"),
        linux_payload("sample", "1.2.2-1", "arm64"),
        linux_payload("sample-common", "1.2.3-1", "all"),
    ];
    let remote = vec![
        linux_version("sample", "1.2.1-1", "amd64"),
        linux_version("sample", "1.2.3-1", "amd64"),
        linux_version("sample", "1.2.3-1", "arm64"),
    ];

    let selected = select_publishable_linux_payloads(local, &remote, compare_version_strings)
        .expect("publish selection should resolve");

    assert_eq!(
        selected,
        vec![
            linux_payload("sample", "1.2.4-1", "amd64"),
            linux_payload("sample-common", "1.2.3-1", "all"),
        ]
    );
}

#[test]
fn publishable_linux_payloads_from_manifest_and_remote_keys_selects_candidates_once() {
    let manifest = linux_manifest(
        PackageSystem::Deb,
        vec![
            linux_artifact("sample", "1.2.4-1", "amd64"),
            linux_artifact("sample", "1.2.2-1", "arm64"),
            linux_artifact("sample-common", "1.2.3-1", "all"),
        ],
    );
    let prefix = genmeta_xtask_release::publish::RemotePrefix::parse("apt/sample")
        .expect("prefix should parse");

    let selected =
        genmeta_xtask_release::publish::publishable_linux_payloads_from_manifest_and_remote_keys(
            &manifest,
            &prefix,
            [
                "apt/sample/pool/main/s/sample/sample_1.2.1-1_amd64.deb",
                "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb",
                "apt/sample/pool/main/s/sample/sample_1.2.3-1_arm64.deb",
            ],
            compare_version_strings,
        )
        .expect("publishable payloads should resolve");

    assert_eq!(
        selected,
        vec![
            linux_payload("sample", "1.2.4-1", "amd64"),
            linux_payload("sample-common", "1.2.3-1", "all"),
        ]
    );
}

#[test]
fn publishable_linux_payloads_from_manifest_and_remote_keys_rejects_non_linux_manifest() {
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: Vec::new(),
    };
    let prefix =
        genmeta_xtask_release::publish::RemotePrefix::parse("brew").expect("prefix should parse");

    let error =
        genmeta_xtask_release::publish::publishable_linux_payloads_from_manifest_and_remote_keys(
            &manifest,
            &prefix,
            ["brew/sample.rb"],
            compare_version_strings,
        )
        .expect_err("brew manifest should fail");

    assert_eq!(
        error.to_string(),
        "failed to read linux package payloads from manifest"
    );
}

#[test]
fn remote_deb_package_entries_from_packages_parse_required_stanza_fields() {
    let entries = genmeta_xtask_release::publish::remote_deb_package_entries_from_packages(
        r#"
Package: sample
Version: 1.2.3-1
Architecture: amd64
Filename: pool/main/s/sample/sample_1.2.3-1_amd64.deb
Description: ignored

Package: sample-common
Version: 1.2.3-1
Architecture: all
Filename: pool/main/s/sample-common/sample-common_1.2.3-1_all.deb
"#,
    )
    .expect("remote package entries should parse");

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].version.package, "sample");
    assert_eq!(entries[0].version.version, "1.2.3-1");
    assert_eq!(entries[0].version.architecture, "amd64");
    assert_eq!(
        entries[0].filename,
        "pool/main/s/sample/sample_1.2.3-1_amd64.deb"
    );
    assert_eq!(entries[1].version.package, "sample-common");
    assert_eq!(entries[1].version.architecture, "all");
}

#[test]
fn remote_deb_package_entries_from_packages_skip_empty_stanzas() {
    let entries = genmeta_xtask_release::publish::remote_deb_package_entries_from_packages(
        "\n\nPackage: sample\nVersion: 1.2.3-1\nArchitecture: amd64\nFilename: pool/main/s/sample/sample_1.2.3-1_amd64.deb\n\n\n",
    )
    .expect("remote package entries should parse");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].version.package, "sample");
}

#[test]
fn remote_deb_package_entries_from_packages_requires_filename() {
    let error = genmeta_xtask_release::publish::remote_deb_package_entries_from_packages(
        "Package: sample\nVersion: 1.2.3-1\nArchitecture: amd64\n",
    )
    .expect_err("missing filename should fail");

    assert_eq!(
        error.to_string(),
        "remote deb package stanza is missing Filename"
    );
}

#[test]
fn publishable_deb_payloads_from_manifest_and_packages_selects_candidates_once() {
    let manifest = linux_manifest(
        PackageSystem::Deb,
        vec![
            linux_artifact("sample", "1.2.4-1", "amd64"),
            linux_artifact("sample", "1.2.2-1", "arm64"),
            linux_artifact("sample-common", "1.2.3-1", "all"),
        ],
    );

    let selected = genmeta_xtask_release::publish::publishable_deb_payloads_from_manifest_and_packages(
        &manifest,
        [
            "Package: sample\nVersion: 1.2.1-1\nArchitecture: amd64\nFilename: pool/main/s/sample/sample_1.2.1-1_amd64.deb\n\nPackage: sample\nVersion: 1.2.3-1\nArchitecture: amd64\nFilename: pool/main/s/sample/sample_1.2.3-1_amd64.deb\n",
            "Package: sample\nVersion: 1.2.3-1\nArchitecture: arm64\nFilename: pool/main/s/sample/sample_1.2.3-1_arm64.deb\n",
        ],
        compare_version_strings,
    )
    .expect("publishable deb payloads should resolve");

    assert_eq!(
        selected,
        vec![
            linux_payload("sample", "1.2.4-1", "amd64"),
            linux_payload("sample-common", "1.2.3-1", "all"),
        ]
    );
}

#[test]
fn retained_remote_linux_package_payloads_exclude_local_manifest_versions() {
    let manifest = linux_manifest(
        PackageSystem::Rpm,
        vec![
            linux_artifact("sample", "1.2.4-1", "x86_64"),
            linux_artifact("sample", "1.2.4-1", "aarch64"),
        ],
    );
    let remote_payloads = vec![
        linux_payload("sample", "1.2.3-1", "x86_64"),
        linux_payload("sample", "1.2.4-1", "x86_64"),
        linux_payload("sample", "1.2.4-1", "aarch64"),
        linux_payload("sample-common", "1.2.4-1", "noarch"),
    ];

    let retained = genmeta_xtask_release::publish::retained_remote_linux_package_payloads(
        &manifest,
        remote_payloads,
    )
    .expect("retained remote linux payloads should resolve");

    assert_eq!(
        retained,
        vec![
            linux_payload("sample", "1.2.3-1", "x86_64"),
            linux_payload("sample-common", "1.2.4-1", "noarch"),
        ]
    );
}

#[test]
fn retained_remote_deb_package_entries_exclude_local_manifest_versions() {
    let manifest = linux_manifest(
        PackageSystem::Deb,
        vec![
            linux_artifact("sample", "1.2.4-1", "amd64"),
            linux_artifact("sample", "1.2.4-1", "arm64"),
        ],
    );
    let remote_entries = vec![
        remote_deb_entry("sample", "1.2.3-1", "amd64"),
        remote_deb_entry("sample", "1.2.4-1", "amd64"),
        remote_deb_entry("sample", "1.2.4-1", "arm64"),
        remote_deb_entry("sample-common", "1.2.4-1", "all"),
    ];

    let retained = genmeta_xtask_release::publish::retained_remote_deb_package_entries(
        &manifest,
        remote_entries,
    )
    .expect("retained remote package entries should resolve");

    assert_eq!(
        retained,
        vec![
            remote_deb_entry("sample", "1.2.3-1", "amd64"),
            remote_deb_entry("sample-common", "1.2.4-1", "all"),
        ]
    );
}

#[test]
fn publishable_deb_payloads_from_manifest_and_packages_rejects_rpm_manifest() {
    let manifest = linux_manifest(
        PackageSystem::Rpm,
        vec![linux_artifact("sample", "1.2.3-1", "x86_64")],
    );

    let error =
        genmeta_xtask_release::publish::publishable_deb_payloads_from_manifest_and_packages(
            &manifest,
            ["Package: sample\nVersion: 1.2.3-1\nArchitecture: x86_64\nFilename: sample.rpm\n"],
            compare_version_strings,
        )
        .expect_err("rpm manifest should fail");

    assert_eq!(
        error.to_string(),
        "rpm package manifest is not a deb package manifest"
    );
}

fn linux_manifest(kind: PackageSystem, artifacts: Vec<PackageArtifact>) -> PackageManifest {
    PackageManifest {
        schema_version: 1,
        kind,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts,
    }
}

fn linux_artifact(package: &str, version: &str, architecture: &str) -> PackageArtifact {
    PackageArtifact {
        target: "x86_64-unknown-linux-gnu".to_string(),
        path: format!("{architecture}/release/{package}-{version}.pkg"),
        sha256: "0".repeat(64),
        size: 1,
        package_name: Some(package.to_string()),
        package_version: Some(version.to_string()),
        architecture: Some(architecture.to_string()),
        archive_name: Some(format!("{package}-{version}.{architecture}.pkg")),
        features: Vec::new(),
        profile: Some("release".to_string()),
    }
}

fn linux_payload(package: &str, version: &str, architecture: &str) -> LinuxPackagePayload {
    LinuxPackagePayload {
        package: package.to_string(),
        version: version.to_string(),
        architecture: architecture.to_string(),
        archive_name: format!("{package}-{version}.{architecture}.pkg"),
        path: format!("{architecture}/release/{package}-{version}.pkg"),
    }
}

fn linux_version(package: &str, version: &str, architecture: &str) -> LinuxPackageVersion {
    LinuxPackageVersion {
        package: package.to_string(),
        version: version.to_string(),
        architecture: architecture.to_string(),
    }
}

fn remote_deb_entry(
    package: &str,
    version: &str,
    architecture: &str,
) -> genmeta_xtask_release::publish::RemoteDebPackageEntry {
    genmeta_xtask_release::publish::RemoteDebPackageEntry {
        version: linux_version(package, version, architecture),
        filename: format!(
            "pool/main/{}/{package}/{package}_{version}_{architecture}.deb",
            &package[..1]
        ),
    }
}

fn compare_version_strings(
    left: &str,
    right: &str,
) -> Result<std::cmp::Ordering, std::convert::Infallible> {
    Ok(left.cmp(right))
}

use genmeta_xtask_release::publish::{
    RemotePayloadState, UploadCondition, plan_immutable_upload, plan_versioned_immutable_payload,
};

#[test]
fn automated_publish_requires_manual_gate_when_remote_surface_is_missing() {
    let decision = genmeta_xtask_release::publish::plan_automated_publish(
        genmeta_xtask_release::publish::RemotePublishSurface::Missing,
        vec!["candidate-payload".to_string()],
    );

    assert_eq!(
        decision,
        genmeta_xtask_release::publish::AutomatedPublishDecision::ManualInitialPublicationRequired
    );
}

#[test]
fn automated_publish_keeps_candidates_when_remote_surface_exists() {
    let decision = genmeta_xtask_release::publish::plan_automated_publish(
        genmeta_xtask_release::publish::RemotePublishSurface::Present,
        vec!["candidate-payload".to_string()],
    );

    assert_eq!(
        decision,
        genmeta_xtask_release::publish::AutomatedPublishDecision::Publish {
            payloads: vec!["candidate-payload".to_string()]
        }
    );
}

#[test]
fn immutable_missing_remote_uploads_only_when_absent() {
    let condition = plan_immutable_upload("system/file.tar.gz", "abc", RemotePayloadState::Missing)
        .expect("missing remote should be publishable");

    assert_eq!(condition, Some(UploadCondition::IfMissing));
}

#[test]
fn immutable_same_hash_remote_is_skipped() {
    let condition = plan_immutable_upload(
        "system/file.tar.gz",
        "abc",
        RemotePayloadState::Present {
            sha256: "abc".to_string(),
        },
    )
    .expect("same hash should be accepted");

    assert_eq!(condition, None);
}

#[test]
fn immutable_different_hash_remote_fails() {
    let error = plan_immutable_upload(
        "system/file.tar.gz",
        "abc",
        RemotePayloadState::Present {
            sha256: "def".to_string(),
        },
    )
    .expect_err("different hash should fail");

    assert_eq!(
        error.to_string(),
        "remote immutable payload system/file.tar.gz already exists with different sha256 def"
    );
}

#[test]
fn versioned_payload_reuses_remote_sha_for_existing_version() {
    let plan = plan_versioned_immutable_payload(
        "system/file.tar.gz",
        "local-sha",
        RemotePayloadState::Present {
            sha256: "published-sha".to_string(),
        },
    );

    assert_eq!(plan.metadata_sha256(), "published-sha");
    assert_eq!(plan.upload_condition(), None);
    assert!(plan.reuses_remote_payload());
    assert!(!plan.remote_sha256_matches_local());
}

use genmeta_xtask_release::publish::s3_publish_env_names;

#[test]
fn s3_deb_publish_env_names_include_common_and_signing_refs() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let names = s3_publish_env_names(&contract, PackageSystem::Deb)
        .expect("deb publish env names should resolve");

    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec![
            "XTASK_RELEASE_APT_SIGNING_FINGERPRINT".to_string(),
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "XTASK_RELEASE_APT_SIGNING_PASSPHRASE".to_string(),
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
        ]
    );
}

#[test]
fn s3_brew_publish_env_names_include_tap_token_without_linux_signing_refs() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let names = s3_publish_env_names(&contract, PackageSystem::Brew)
        .expect("brew publish env names should resolve");

    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec![
            "HOMEBREW_TAP_GITHUB_TOKEN".to_string(),
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
        ]
    );
}

#[test]
fn s3_publish_env_names_reject_missing_destination_branch() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let error = s3_publish_env_names(&contract, PackageSystem::Scoop)
        .expect_err("missing scoop branch should fail");

    assert_eq!(error.to_string(), "destination s3 scoop branch is missing");
}

use genmeta_xtask_release::publish::{S3PublishTarget, resolve_s3_publish_target};

#[test]
fn s3_brew_publish_target_resolves_from_destination_s3_branch() {
    let contract: ReleaseContract =
        toml::from_str(GMUTILS_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        ("HOMEBREW_TAP_GITHUB_TOKEN".to_string(), "token".to_string()),
    ]);

    let target = resolve_s3_publish_target(&contract, PackageSystem::Brew, &values)
        .expect("brew target should resolve");

    assert_eq!(target.bucket(), "download");
    assert_eq!(target.endpoint_url(), "https://r2.example");
    match target {
        S3PublishTarget::Brew(target) => {
            assert_eq!(target.prefix.as_str(), "homebrew");
            assert_eq!(
                target.public_base_url.as_str(),
                "https://download.dhttp.net/homebrew"
            );
            assert_eq!(target.tap.repository, "genmeta/homebrew-genmeta");
            assert_eq!(target.tap.token, "token");
        }
        _ => panic!("expected brew target"),
    }
}

#[test]
fn s3_deb_publish_target_requires_fingerprint_env() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "key".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_PASSPHRASE".to_string(),
            "passphrase".to_string(),
        ),
    ]);

    let error = resolve_s3_publish_target(&contract, PackageSystem::Deb, &values)
        .expect_err("missing fingerprint should fail");

    assert_eq!(
        error.to_string(),
        "missing required release environment variable XTASK_RELEASE_APT_SIGNING_FINGERPRINT"
    );
}

#[test]
fn write_manifest_refuses_existing_manifest_without_overwrite() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let mut manifest = writable_manifest("1.2.3");
    write_manifest(&target_dir, &manifest, false).expect("initial manifest should write");

    manifest.version = "1.2.4".to_string();
    let error = write_manifest(&target_dir, &manifest, false)
        .expect_err("existing manifest should require overwrite");

    assert!(
        error
            .to_string()
            .contains("package manifest already exists")
    );
    let loaded = load_manifest(&target_dir, PackageSystem::Brew).expect("manifest should load");
    assert_eq!(loaded.version, "1.2.3");
}

#[test]
fn write_manifest_overwrites_existing_manifest_when_allowed() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let mut manifest = writable_manifest("1.2.3");
    write_manifest(&target_dir, &manifest, false).expect("initial manifest should write");

    manifest.version = "1.2.4".to_string();
    write_manifest(&target_dir, &manifest, true).expect("overwrite should write");

    let loaded = load_manifest(&target_dir, PackageSystem::Brew).expect("manifest should load");
    assert_eq!(loaded.version, "1.2.4");
}

#[test]
fn write_package_command_manifest_uses_command_overwrite_flag() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let command = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            std::ffi::OsString::from("--overwrite-manifest"),
            std::ffi::OsString::from("brew"),
            std::ffi::OsString::from("--target"),
            std::ffi::OsString::from("aarch64-apple-darwin"),
        ],
    )
    .expect("package command should parse");
    let temp = tempfile::tempdir().expect("tempdir should create");
    let target_dir = temp.path().join("target");
    let mut manifest = writable_manifest("0.5.1");
    manifest.package = "pishoo".to_string();
    manifest.artifacts[0].target = "aarch64-apple-darwin".to_string();
    manifest.artifacts[0].path =
        "aarch64-apple-darwin/release/brew/pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string();
    manifest.artifacts[0].archive_name =
        Some("pishoo-0.5.1-aarch64-apple-darwin.tar.gz".to_string());
    write_manifest(&target_dir, &manifest, false).expect("initial manifest should write");

    manifest.version = "0.5.2".to_string();
    genmeta_xtask_release::manifest::write_package_command_manifest(
        &target_dir,
        &manifest,
        &contract,
        &command,
    )
    .expect("command overwrite should allow manifest write");

    let loaded = load_manifest(&target_dir, PackageSystem::Brew).expect("manifest should load");
    assert_eq!(loaded.version, "0.5.2");
}

fn writable_manifest(version: &str) -> PackageManifest {
    PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "sample".to_string(),
        version: version.to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "aarch64-apple-darwin".to_string(),
            path: "aarch64-apple-darwin/release/brew/sample.tar.gz".to_string(),
            sha256: "0".repeat(64),
            size: 0,
            package_name: None,
            package_version: None,
            architecture: None,
            archive_name: Some("sample.tar.gz".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    }
}

#[test]
fn load_release_contract_reads_and_validates_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("release.toml");
    std::fs::write(&path, include_str!("fixtures/gmutils.release.toml"))
        .expect("contract should write");

    let contract = load_release_contract(&path).expect("contract should load");

    assert!(contract.package("gmutils").is_some());
}

#[test]
fn load_release_contract_rejects_invalid_contract_file() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let path = temp.path().join("release.toml");
    std::fs::write(
        &path,
        r#"
[package.invalid]
version = "1.2.3-1"
description = "Invalid"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#,
    )
    .expect("contract should write");

    let error = load_release_contract(&path).expect_err("invalid contract should fail");

    assert!(error.to_string().contains("invalid release contract"));
}

use genmeta_xtask_release::template::{render_template, ruby_string};

#[test]
fn template_renders_named_variables_with_whitespace() {
    let variables = std::collections::BTreeMap::from([
        ("package.name".to_string(), "sample".to_string()),
        ("package.version".to_string(), "1.2.3".to_string()),
    ]);

    let rendered = render_template("{{ package.name }} {{package.version}}", &variables)
        .expect("template should render");

    assert_eq!(rendered, "sample 1.2.3");
}

#[test]
fn template_rejects_missing_variables() {
    let error = render_template("{{package.name}}", &std::collections::BTreeMap::new())
        .expect_err("missing variable should fail");

    assert_eq!(
        error.to_string(),
        "template variable package.name is not defined"
    );
}

#[test]
fn template_rejects_unclosed_placeholder() {
    let variables = std::collections::BTreeMap::from([("name".to_string(), "sample".to_string())]);

    let error = render_template("{{name", &variables).expect_err("placeholder should fail");

    assert_eq!(
        error.to_string(),
        "template contains unresolved placeholders"
    );
}

#[test]
fn ruby_string_escapes_backslash_and_quote() {
    assert_eq!(ruby_string("a\\\"b"), "a\\\\\\\"b");
}

use genmeta_xtask_release::{
    package::{PackageId, ResolvedPackageMetadata},
    template::package_template_variables,
};

#[test]
fn build_feature_variables_are_derived_from_manifest_artifacts() {
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "sample".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![
            PackageArtifact {
                target: "aarch64-apple-darwin".to_string(),
                path: "aarch64/release/sample.tar.gz".to_string(),
                sha256: "a".to_string(),
                size: 1,
                package_name: None,
                package_version: None,
                architecture: None,
                archive_name: Some("sample-aarch64.tar.gz".to_string()),
                features: vec!["beta".to_string(), "alpha".to_string()],
                profile: Some("release".to_string()),
            },
            PackageArtifact {
                target: "x86_64-apple-darwin".to_string(),
                path: "x86_64/release/sample.tar.gz".to_string(),
                sha256: "b".to_string(),
                size: 1,
                package_name: None,
                package_version: None,
                architecture: None,
                archive_name: Some("sample-x86_64.tar.gz".to_string()),
                features: vec!["alpha".to_string()],
                profile: Some("release".to_string()),
            },
        ],
    };

    let variables = genmeta_xtask_release::template::build_feature_variables(&manifest);

    assert_eq!(
        variables.get("build.features.csv").map(String::as_str),
        Some("alpha,beta")
    );
    assert_eq!(
        variables
            .get("build.features.ruby_array")
            .map(String::as_str),
        Some("[\"alpha\", \"beta\"]")
    );
}

#[test]
fn package_template_variables_use_package_id_and_resolved_metadata() {
    let package_id = PackageId::new("gmutils").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample \"tool\"".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: Some("https://github.com/genmeta/sample".to_string()),
    };

    let variables = package_template_variables(&package_id, &metadata);

    assert_eq!(
        variables.get("package.name").map(String::as_str),
        Some("gmutils")
    );
    assert_eq!(
        variables.get("package.version").map(String::as_str),
        Some("1.2.3")
    );
    assert_eq!(
        variables.get("package.description").map(String::as_str),
        Some("Sample \\\"tool\\\"")
    );
    assert_eq!(
        variables.get("package.license").map(String::as_str),
        Some("Apache-2.0")
    );
    assert_eq!(
        variables.get("package.homepage").map(String::as_str),
        Some("https://dhttp.net")
    );
}

use genmeta_xtask_release::brew::brew_template_variables;

#[test]
fn brew_template_variables_encode_formula_class_and_urls() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample tool".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: None,
    };
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Brew,
        package: "sample-tool".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![
            PackageArtifact {
                target: "aarch64-apple-darwin".to_string(),
                path: "aarch64-apple-darwin/release/brew/sample-tool.tar.gz".to_string(),
                sha256: "arm-sha".to_string(),
                size: 1,
                package_name: None,
                package_version: None,
                architecture: None,
                archive_name: Some("sample-tool-aarch64.tar.gz".to_string()),
                features: vec!["beta".to_string(), "alpha".to_string()],
                profile: Some("release".to_string()),
            },
            PackageArtifact {
                target: "x86_64-apple-darwin".to_string(),
                path: "x86_64-apple-darwin/release/brew/sample-tool.tar.gz".to_string(),
                sha256: "intel-sha".to_string(),
                size: 1,
                package_name: None,
                package_version: None,
                architecture: None,
                archive_name: Some("sample-tool-x86_64.tar.gz".to_string()),
                features: vec!["alpha".to_string()],
                profile: Some("release".to_string()),
            },
        ],
    };
    let base =
        PublicBaseUrl::parse("https://download.example/brew/").expect("base url should parse");

    let variables = brew_template_variables(&package_id, &metadata, &manifest, &base)
        .expect("variables should resolve");

    assert_eq!(
        variables.get("brew.class").map(String::as_str),
        Some("SampleTool")
    );
    assert_eq!(
        variables.get("package.version").map(String::as_str),
        Some("1.2.3")
    );
    assert_eq!(
        variables.get("brew.urls").map(String::as_str),
        Some(
            "  on_arm do\n    url \"https://download.example/brew/sample-tool-aarch64.tar.gz\"\n    sha256 \"arm-sha\"\n  end\n\n  on_intel do\n    url \"https://download.example/brew/sample-tool-x86_64.tar.gz\"\n    sha256 \"intel-sha\"\n  end"
        )
    );
    assert_eq!(
        variables.get("build.features.csv").map(String::as_str),
        Some("alpha,beta")
    );
    assert_eq!(
        variables
            .get("build.features.ruby_array")
            .map(String::as_str),
        Some("[\"alpha\", \"beta\"]")
    );
}

#[test]
fn brew_template_variables_reject_non_brew_manifest() {
    let package_id = PackageId::new("sample").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: None,
    };
    let mut manifest = writable_manifest("1.2.3");
    manifest.kind = PackageSystem::Deb;
    let base =
        PublicBaseUrl::parse("https://download.example/brew").expect("base url should parse");

    let error = brew_template_variables(&package_id, &metadata, &manifest, &base)
        .expect_err("non-brew manifest should fail");

    assert_eq!(
        error.to_string(),
        "brew formula requires brew package manifest"
    );
}

#[test]
fn brew_template_variables_require_archive_name() {
    let package_id = PackageId::new("sample").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: None,
    };
    let mut manifest = writable_manifest("1.2.3");
    manifest.artifacts[0].archive_name = None;
    let base =
        PublicBaseUrl::parse("https://download.example/brew").expect("base url should parse");

    let error = brew_template_variables(&package_id, &metadata, &manifest, &base)
        .expect_err("missing archive name should fail");

    assert_eq!(
        error.to_string(),
        "brew package artifact is missing archive name"
    );
}

use genmeta_xtask_release::publish::mutable_entry_names;

#[test]
fn brew_mutable_entry_names_use_package_id_and_source_version() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let names = mutable_entry_names(&package_id, PackageSystem::Brew, &version)
        .expect("brew names should resolve");

    assert_eq!(names.latest, "sample-tool.rb");
    assert_eq!(names.versioned, "sample-tool-1.2.3.rb");
}

#[test]
fn scoop_mutable_entry_names_use_package_id_and_source_version() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let names = mutable_entry_names(&package_id, PackageSystem::Scoop, &version)
        .expect("scoop names should resolve");

    assert_eq!(names.latest, "sample-tool.json");
    assert_eq!(names.versioned, "sample-tool-1.2.3.json");
}

#[test]
fn brew_mutable_entry_remote_keys_use_prefix_package_id_and_source_version() {
    let prefix =
        genmeta_xtask_release::publish::RemotePrefix::parse("brew").expect("prefix should parse");
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let keys = genmeta_xtask_release::publish::mutable_entry_remote_keys(
        &prefix,
        &package_id,
        PackageSystem::Brew,
        &version,
    )
    .expect("brew entry keys should resolve");

    assert_eq!(keys.latest, "brew/sample-tool.rb");
    assert_eq!(keys.versioned, "brew/sample-tool-1.2.3.rb");
}

#[test]
fn scoop_mutable_entry_remote_keys_use_prefix_package_id_and_source_version() {
    let prefix =
        genmeta_xtask_release::publish::RemotePrefix::parse("scoop").expect("prefix should parse");
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let keys = genmeta_xtask_release::publish::mutable_entry_remote_keys(
        &prefix,
        &package_id,
        PackageSystem::Scoop,
        &version,
    )
    .expect("scoop entry keys should resolve");

    assert_eq!(keys.latest, "scoop/sample-tool.json");
    assert_eq!(keys.versioned, "scoop/sample-tool-1.2.3.json");
}

#[test]
fn mutable_entry_remote_keys_reject_linux_package_systems() {
    let prefix =
        genmeta_xtask_release::publish::RemotePrefix::parse("apt").expect("prefix should parse");
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let error = genmeta_xtask_release::publish::mutable_entry_remote_keys(
        &prefix,
        &package_id,
        PackageSystem::Deb,
        &version,
    )
    .expect_err("deb entry keys should fail");

    assert_eq!(error.to_string(), "failed to resolve mutable entry names");
}

#[test]
fn publish_upload_order_places_payloads_before_mutable_entries() {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Upload {
        key: &'static str,
        entry: bool,
    }

    let mut uploads = vec![
        Upload {
            key: "brew/sample-tool.rb",
            entry: true,
        },
        Upload {
            key: "brew/sample-tool-1.2.3-aarch64.tar.gz",
            entry: false,
        },
        Upload {
            key: "brew/sample-tool-1.2.3.rb",
            entry: true,
        },
        Upload {
            key: "brew/sample-tool-1.2.3-x86_64.tar.gz",
            entry: false,
        },
    ];

    uploads.sort_by_key(|upload| {
        (
            genmeta_xtask_release::publish::publish_upload_order(upload.entry),
            upload.key,
        )
    });

    assert_eq!(
        uploads
            .into_iter()
            .map(|upload| upload.key)
            .collect::<Vec<_>>(),
        vec![
            "brew/sample-tool-1.2.3-aarch64.tar.gz",
            "brew/sample-tool-1.2.3-x86_64.tar.gz",
            "brew/sample-tool-1.2.3.rb",
            "brew/sample-tool.rb",
        ]
    );
}

#[test]
fn apply_mutable_entry_conditions_updates_only_entry_uploads() {
    use genmeta_xtask_release::publish::UploadCondition;

    let uploads = vec![
        genmeta_xtask_release::publish::PublishUploadPlan {
            key: "apt/sample/pool/main/s/sample/sample_1.2.3-1_amd64.deb".to_string(),
            entry: false,
            condition: Some(UploadCondition::IfMissing),
        },
        genmeta_xtask_release::publish::PublishUploadPlan {
            key: "apt/sample/dists/sample/InRelease".to_string(),
            entry: true,
            condition: None,
        },
    ];
    let conditions = std::collections::BTreeMap::from([(
        "apt/sample/dists/sample/InRelease".to_string(),
        UploadCondition::IfMatch("etag".to_string()),
    )]);

    let planned =
        genmeta_xtask_release::publish::apply_mutable_entry_conditions(uploads, &conditions)
            .expect("entry conditions should apply");

    assert_eq!(planned[0].condition, Some(UploadCondition::IfMissing));
    assert_eq!(
        planned[1].condition,
        Some(UploadCondition::IfMatch("etag".to_string()))
    );
}

#[test]
fn apply_mutable_entry_conditions_rejects_missing_entry_baseline() {
    let uploads = vec![genmeta_xtask_release::publish::PublishUploadPlan {
        key: "apt/sample/dists/sample/InRelease".to_string(),
        entry: true,
        condition: None,
    }];
    let conditions = std::collections::BTreeMap::new();

    let error =
        genmeta_xtask_release::publish::apply_mutable_entry_conditions(uploads, &conditions)
            .expect_err("missing entry condition should fail");

    assert_eq!(
        error.to_string(),
        "mutable entry upload apt/sample/dists/sample/InRelease is missing remote baseline condition"
    );
}

#[test]
fn linux_package_systems_do_not_have_package_entry_names() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let version = semver::Version::parse("1.2.3").unwrap();

    let error = mutable_entry_names(&package_id, PackageSystem::Deb, &version)
        .expect_err("deb should not have package entry names");

    assert_eq!(
        error.to_string(),
        "deb does not define per-package mutable entry files"
    );
}

use genmeta_xtask_release::scoop::{render_scoop_json, scoop_template_variables};

#[test]
fn scoop_branch_bin_is_package_system_contract_data() {
    let input = r#"
[package.sample-tool]
version = "1.2.3"
description = "Sample tool"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample-tool.scoop]
bin = ["sample-tool.exe", "sample-helper.bat"]

[package.sample-tool.scoop.build]
script = "build-scoop.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");

    let branch = contract
        .package("sample-tool")
        .and_then(|package| package.scoop.as_ref())
        .expect("scoop branch should exist");
    assert_eq!(branch.bin, vec!["sample-tool.exe", "sample-helper.bat"]);
}

#[test]
fn scoop_template_variables_expose_manifest_and_branch_values() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample tool".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: None,
    };
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Scoop,
        package: "sample-tool".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-pc-windows-msvc".to_string(),
            path: "x86_64-pc-windows-msvc/release/scoop/sample-tool.zip".to_string(),
            sha256: "sample-sha".to_string(),
            size: 1,
            package_name: None,
            package_version: None,
            architecture: None,
            archive_name: Some("sample-tool-x86_64.zip".to_string()),
            features: vec!["alpha".to_string()],
            profile: Some("release".to_string()),
        }],
    };
    let base = PublicBaseUrl::parse("https://download.example/scoop")
        .expect("public base url should parse");

    let variables = scoop_template_variables(
        &package_id,
        &metadata,
        &manifest,
        &base,
        &["sample-tool.exe".to_string()],
    )
    .expect("scoop template variables should resolve");

    assert_eq!(
        variables.get("package.name").map(String::as_str),
        Some("sample-tool")
    );
    assert_eq!(
        variables.get("build.features.csv").map(String::as_str),
        Some("alpha")
    );
    assert_eq!(
        variables.get("scoop.bin.json").map(String::as_str),
        Some(
            "[
  \"sample-tool.exe\"
]"
        )
    );
    assert_eq!(
        variables.get("scoop.architecture.json").map(String::as_str),
        Some(
            "{
  \"64bit\": {
    \"hash\": \"sample-sha\",
    \"url\": \"https://download.example/scoop/sample-tool-x86_64.zip\"
  }
}"
        )
    );
}

#[test]
fn render_scoop_json_uses_branch_bin_and_public_base_url() {
    let package_id = PackageId::new("sample-tool").expect("package id should parse");
    let metadata = ResolvedPackageMetadata {
        source_version: semver::Version::parse("1.2.3").unwrap(),
        description: "Sample tool".to_string(),
        license: "Apache-2.0".to_string(),
        homepage: "https://dhttp.net".to_string(),
        repository: None,
    };
    let manifest = PackageManifest {
        schema_version: 1,
        kind: PackageSystem::Scoop,
        package: "sample-tool".to_string(),
        version: "1.2.3".to_string(),
        generated_at: "2026-06-29T00:00:00Z".to_string(),
        git_commit: None,
        git_dirty: false,
        artifacts: vec![PackageArtifact {
            target: "x86_64-pc-windows-msvc".to_string(),
            path: "x86_64-pc-windows-msvc/release/scoop/sample-tool.zip".to_string(),
            sha256: "sample-sha".to_string(),
            size: 1,
            package_name: None,
            package_version: None,
            architecture: None,
            archive_name: Some("sample-tool-x86_64.zip".to_string()),
            features: Vec::new(),
            profile: Some("release".to_string()),
        }],
    };
    let base = PublicBaseUrl::parse("https://download.example/scoop")
        .expect("public base url should parse");

    let json = render_scoop_json(
        &package_id,
        &metadata,
        &manifest,
        &base,
        &[
            "sample-tool.exe".to_string(),
            "sample-helper.bat".to_string(),
        ],
    )
    .expect("scoop json should render");
    let value: serde_json::Value = serde_json::from_str(&json).expect("json should parse");

    assert_eq!(value["version"], "1.2.3");
    assert_eq!(value["description"], "Sample tool");
    assert_eq!(value["bin"][0], "sample-tool.exe");
    assert_eq!(value["bin"][1], "sample-helper.bat");
    assert_eq!(
        value["architecture"]["64bit"]["url"],
        "https://download.example/scoop/sample-tool-x86_64.zip"
    );
    assert_eq!(value["architecture"]["64bit"]["hash"], "sample-sha");
    assert_eq!(
        value["checkver"]["url"],
        "https://download.example/scoop/sample-tool.json"
    );
    assert_eq!(
        value["autoupdate"]["64bit"]["url"],
        "https://download.example/scoop/sample-tool-x86_64.zip"
    );
}

use genmeta_xtask_release::publish::{PublicBaseUrl, RemotePrefix};

#[test]
fn remote_prefix_trims_slashes_and_joins_keys() {
    let prefix = RemotePrefix::parse("/homebrew/").expect("prefix should parse");

    assert_eq!(prefix.as_str(), "homebrew");
    assert_eq!(prefix.join("sample.rb"), "homebrew/sample.rb");
}

#[test]
fn public_base_url_trims_trailing_slashes_and_joins_paths() {
    let base = PublicBaseUrl::parse("https://download.example/homebrew///")
        .expect("public base url should parse");

    assert_eq!(base.as_str(), "https://download.example/homebrew");
    assert_eq!(
        base.join("sample.tar.gz"),
        "https://download.example/homebrew/sample.tar.gz"
    );
}

#[test]
fn s3_publish_target_rejects_empty_prefix() {
    let input = r#"
[package.sample]
version = "1.2.3"
description = "Sample"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.brew]
template = "sample.rb.in"

[package.sample.brew.build]
script = "build-brew.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.brew]
prefix = "///"
public_base_url = "https://download.example/homebrew"
tap.repository = "genmeta/homebrew-genmeta"
tap.base_branch = "main"
tap.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        ("HOMEBREW_TAP_GITHUB_TOKEN".to_string(), "token".to_string()),
    ]);

    let error = resolve_s3_publish_target(&contract, PackageSystem::Brew, &values)
        .expect_err("empty prefix should fail");

    assert_eq!(error.to_string(), "invalid remote prefix");
}

#[test]
fn destination_s3_branch_must_have_matching_package_system_branch() {
    let input = r#"
[package.sample]
version = "1.0.0"
description = "Sample package"
license = "Apache-2.0"
homepage = "https://dhttp.net"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.scoop]
prefix = "scoop"
public_base_url = "https://download.dhttp.net/scoop"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("orphan destination branch should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 scoop branch has no package scoop branch"
    );
}

#[test]
fn container_build_env_binding_mounts_env_source_and_uses_container_path() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.build.env.ROOT_CA]
env = "ROOT_CA"
container_path = "/container/root.crt"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[package.sample.deb.build.container]
image = "linux-builder"

[package.sample.brew]
template = "xtask/templates/sample.rb.in"

[package.sample.brew.build]
script = "xtask/release/brew/sample.sh"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;
    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values =
        std::collections::BTreeMap::from([("ROOT_CA".to_string(), "/host/root.crt".to_string())]);

    let deb = genmeta_xtask_release::plan::build_invocation_for_profile_with_env_values(
        &contract,
        "sample",
        PackageSystem::Deb,
        RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string()),
        genmeta_xtask_release::system::BuildProfile::Release,
        &[],
        &values,
    )
    .expect("container build should plan");

    assert_eq!(
        deb.env.get("ROOT_CA").map(String::as_str),
        Some("/container/root.crt")
    );
    assert_eq!(
        deb.env_mounts,
        [genmeta_xtask_release::plan::PlannedEnvMount {
            source: std::path::PathBuf::from("/host/root.crt"),
            destination: std::path::PathBuf::from("/container/root.crt"),
            read_only: true,
        }]
    );

    let brew = genmeta_xtask_release::plan::build_invocation_for_profile_with_env_values(
        &contract,
        "sample",
        PackageSystem::Brew,
        RequestedTarget::Triple("aarch64-apple-darwin".to_string()),
        genmeta_xtask_release::system::BuildProfile::Release,
        &[],
        &values,
    )
    .expect("non-container build should plan");

    assert_eq!(
        brew.env.get("ROOT_CA").map(String::as_str),
        Some("/host/root.crt")
    );
    assert!(brew.env_mounts.is_empty());
}

#[test]
fn value_build_env_binding_must_not_set_container_path() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.build.env.ROOT_CA]
value = "/host/root.crt"
container_path = "/container/root.crt"

[package.sample.deb]
revision = "1"
architecture = "target"

[package.sample.deb.build]
script = "xtask/release/deb/sample.sh"

[package.sample.deb.build.container]
image = "linux-builder"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"
"#;

    let contract: ReleaseContract = toml::from_str(input).expect("contract should parse");
    let error = contract
        .validate()
        .expect_err("value-backed container path should fail");

    assert_eq!(
        error.to_string(),
        "package sample env binding ROOT_CA container path requires env"
    );
}

#[test]
fn s3_deb_publish_target_allows_missing_local_signing_passphrase() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "key".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_FINGERPRINT".to_string(),
            "fingerprint".to_string(),
        ),
    ]);

    let target = resolve_s3_publish_target(&contract, PackageSystem::Deb, &values)
        .expect("deb publish target should allow missing local passphrase");

    match target {
        S3PublishTarget::Deb(target) => {
            assert_eq!(target.signing_key, "key");
            assert_eq!(target.signing_passphrase, None);
            assert_eq!(target.fingerprint, "fingerprint");
        }
        _ => panic!("expected deb target"),
    }
}

#[test]
fn s3_deb_publish_target_rejects_empty_signing_passphrase() {
    let contract: ReleaseContract =
        toml::from_str(GATEWAY_CONTRACT).expect("contract should parse");
    contract.validate().expect("contract should validate");
    let values = std::collections::BTreeMap::from([
        (
            "XTASK_RELEASE_S3_ENDPOINT_URL".to_string(),
            "https://r2.example".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_ACCESS_KEY_ID".to_string(),
            "access".to_string(),
        ),
        (
            "XTASK_RELEASE_S3_SECRET_ACCESS_KEY".to_string(),
            "secret".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_KEY".to_string(),
            "key".to_string(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_PASSPHRASE".to_string(),
            String::new(),
        ),
        (
            "XTASK_RELEASE_APT_SIGNING_FINGERPRINT".to_string(),
            "fingerprint".to_string(),
        ),
    ]);

    let error = resolve_s3_publish_target(&contract, PackageSystem::Deb, &values)
        .expect_err("empty passphrase should fail");

    assert_eq!(
        error.to_string(),
        "release environment variable XTASK_RELEASE_APT_SIGNING_PASSPHRASE must not be empty"
    );
}
