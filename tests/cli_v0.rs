use std::ffi::OsString;

use genmeta_xtask_release::{cli::parse_package_system_sections, system::PackageSystem};

fn os(value: &str) -> OsString {
    OsString::from(value)
}

#[test]
fn parses_grouped_package_system_sections() {
    let sections = parse_package_system_sections(&[
        os("deb"),
        os("--target"),
        os("x86_64-unknown-linux-gnu"),
        os("--target"),
        os("aarch64-unknown-linux-gnu"),
        os("rpm"),
        os("--target"),
        os("common"),
        os("brew"),
        os("--target"),
        os("aarch64-apple-darwin"),
    ])
    .expect("grouped package-system sections should parse");

    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].system, PackageSystem::Deb);
    assert_eq!(
        sections[0].args,
        [
            os("--target"),
            os("x86_64-unknown-linux-gnu"),
            os("--target"),
            os("aarch64-unknown-linux-gnu"),
        ]
    );
    assert_eq!(sections[1].system, PackageSystem::Rpm);
    assert_eq!(sections[1].args, [os("--target"), os("common")]);
    assert_eq!(sections[2].system, PackageSystem::Brew);
    assert_eq!(
        sections[2].args,
        [os("--target"), os("aarch64-apple-darwin")]
    );
}

#[test]
fn preserves_repeated_options_inside_package_system_section() {
    let sections = parse_package_system_sections(&[
        os("scoop"),
        os("--target"),
        os("x86_64-pc-windows-msvc"),
        os("--target"),
        os("i686-pc-windows-msvc"),
        os("--sibling"),
        os("/workspace/dhttp"),
        os("--sibling"),
        os("/workspace/h3x"),
    ])
    .expect("repeated options should remain scoped to the section");

    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].system, PackageSystem::Scoop);
    assert_eq!(
        sections[0].args,
        [
            os("--target"),
            os("x86_64-pc-windows-msvc"),
            os("--target"),
            os("i686-pc-windows-msvc"),
            os("--sibling"),
            os("/workspace/dhttp"),
            os("--sibling"),
            os("/workspace/h3x"),
        ]
    );
}

#[test]
fn rejects_arguments_before_first_package_system() {
    let error = parse_package_system_sections(&[os("--target"), os("x86_64-unknown-linux-gnu")])
        .expect_err("arguments before a package-system section should fail");

    assert_eq!(
        error.to_string(),
        "expected a package system before argument --target"
    );
}

#[test]
fn rejects_unknown_package_system() {
    let error = parse_package_system_sections(&[os("homebrew")])
        .expect_err("unknown package system should fail");

    assert_eq!(error.to_string(), "unknown package system homebrew");
}

#[test]
fn parses_package_section_args_without_restricting_target_or_feature_values() {
    let args = genmeta_xtask_release::cli::parse_package_section_args(&[
        os("--target"),
        os("common"),
        os("--target"),
        os("aarch64-unknown-linux-gnu"),
        os("--features"),
        os("sshd,pam"),
        os("--sibling"),
        os("/workspace/dhttp"),
        os("--debug"),
    ])
    .expect("package section args should parse");

    assert_eq!(
        args.targets,
        [
            genmeta_xtask_release::system::RequestedTarget::Common,
            genmeta_xtask_release::system::RequestedTarget::Triple(
                "aarch64-unknown-linux-gnu".to_string()
            ),
        ]
    );
    assert_eq!(args.features, ["sshd".to_string(), "pam".to_string()]);
    assert_eq!(
        args.siblings,
        [std::path::PathBuf::from("/workspace/dhttp")]
    );
    assert!(args.debug);
}

#[test]
fn package_section_args_parse_explicit_sibling_sources_and_patch_overrides() {
    let args = genmeta_xtask_release::cli::parse_package_section_args(&[
        os("--target"),
        os("x86_64-unknown-linux-gnu"),
        os("--sibling"),
        os("dhttp=/workspace/dhttp"),
        os("--patch"),
        os("crates-io"),
        os("dhttp"),
        os("dhttp/dhttp"),
        os("--patch"),
        os("https://github.com/genmeta/dhttp.git"),
        os("dhttp-access"),
        os("dhttp/access"),
    ])
    .expect("package section args should parse explicit sibling overlays");

    assert_eq!(
        args.sibling_sources,
        [genmeta_xtask_release::sibling::SiblingSource {
            name: "dhttp".to_string(),
            host_path: std::path::PathBuf::from("/workspace/dhttp"),
        }]
    );
    assert_eq!(
        args.patches,
        [
            genmeta_xtask_release::sibling::PatchOverride {
                source: genmeta_xtask_release::sibling::PatchSource::CratesIo,
                package: "dhttp".to_string(),
                sibling: "dhttp".to_string(),
                relative_path: std::path::PathBuf::from("dhttp"),
            },
            genmeta_xtask_release::sibling::PatchOverride {
                source: genmeta_xtask_release::sibling::PatchSource::Git(
                    "https://github.com/genmeta/dhttp.git".to_string(),
                ),
                package: "dhttp-access".to_string(),
                sibling: "dhttp".to_string(),
                relative_path: std::path::PathBuf::from("access"),
            },
        ]
    );
}

#[test]
fn package_section_args_render_explicit_sibling_overlay_plan() {
    let args = genmeta_xtask_release::cli::parse_package_section_args(&[
        os("--target"),
        os("x86_64-unknown-linux-gnu"),
        os("--sibling"),
        os("dhttp=/workspace/dhttp"),
        os("--patch"),
        os("crates-io"),
        os("dhttp"),
        os("dhttp/dhttp"),
    ])
    .expect("package section args should parse explicit sibling overlays");

    let overlay = args
        .container_overlay_plan()
        .expect("explicit sibling overlay should plan")
        .expect("explicit sibling overlay should exist");

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

#[test]
fn package_section_args_require_targets() {
    let error =
        genmeta_xtask_release::cli::parse_package_section_args(&[os("--features"), os("sshd")])
            .expect_err("missing targets should fail");

    let report = snafu::Report::from_error(&error).to_string();
    assert!(report.contains("required"));
    assert!(report.contains("--target"));
}

#[test]
fn package_section_args_reject_unknown_option() {
    let error = genmeta_xtask_release::cli::parse_package_section_args(&[
        os("--target"),
        os("x86_64-unknown-linux-gnu"),
        os("--install-pam"),
    ])
    .expect_err("unknown package section option should fail");

    let report = snafu::Report::from_error(&error).to_string();
    assert!(report.contains("unexpected argument '--install-pam'"));
}

#[test]
fn package_build_requests_are_limited_to_package_system_branches_in_contract() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gateway.release.toml"))
            .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let requests = genmeta_xtask_release::cli::parse_package_build_requests(
        &contract,
        &[
            os("deb"),
            os("--target"),
            os("common"),
            os("brew"),
            os("--target"),
            os("aarch64-apple-darwin"),
        ],
    )
    .expect("package build requests should parse");

    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].system, PackageSystem::Deb);
    assert_eq!(
        requests[0].args.targets,
        [genmeta_xtask_release::system::RequestedTarget::Common]
    );
    assert_eq!(requests[1].system, PackageSystem::Brew);
}

#[test]
fn package_build_requests_reject_package_system_without_contract_branch() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gateway.release.toml"))
            .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let error = genmeta_xtask_release::cli::parse_package_build_requests(
        &contract,
        &[os("scoop"), os("--target"), os("x86_64-pc-windows-msvc")],
    )
    .expect_err("scoop should fail because gateway has no scoop branch");

    assert_eq!(
        error.to_string(),
        "package system scoop is not defined by any package branch"
    );
}

#[test]
fn package_command_request_parses_overwrite_manifest_before_package_systems() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gateway.release.toml"))
            .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let request = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[
            os("--overwrite-manifest"),
            os("deb"),
            os("--target"),
            os("common"),
            os("--target"),
            os("x86_64-unknown-linux-gnu"),
            os("rpm"),
            os("--target"),
            os("x86_64-unknown-linux-gnu"),
        ],
    )
    .expect("package command request should parse");

    assert!(request.overwrite_manifest);
    assert_eq!(request.builds.len(), 2);
    assert_eq!(request.builds[0].system, PackageSystem::Deb);
    assert_eq!(
        request.builds[0].args.targets,
        [
            genmeta_xtask_release::system::RequestedTarget::Common,
            genmeta_xtask_release::system::RequestedTarget::Triple(
                "x86_64-unknown-linux-gnu".to_string()
            ),
        ]
    );
    assert_eq!(request.builds[1].system, PackageSystem::Rpm);
}

#[test]
fn package_command_request_defaults_overwrite_manifest_to_false() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let request = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[os("scoop"), os("--target"), os("x86_64-pc-windows-msvc")],
    )
    .expect("package command request should parse");

    assert!(!request.overwrite_manifest);
    assert_eq!(request.builds.len(), 1);
    assert_eq!(request.builds[0].system, PackageSystem::Scoop);
}

#[test]
fn package_command_request_rejects_unknown_global_option() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let error = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[os("--dry-run"), os("deb"), os("--target"), os("common")],
    )
    .expect_err("unknown global package option should fail");

    let report = snafu::Report::from_error(&error).to_string();
    assert!(report.contains("expected a package system before argument --dry-run"));
}

#[test]
fn package_command_request_preserves_unknown_package_system_error() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let error = genmeta_xtask_release::cli::parse_package_command_request(
        &contract,
        &[os("homebrew"), os("--target"), os("aarch64-apple-darwin")],
    )
    .expect_err("unknown package system should fail through package-system parsing");

    let report = snafu::Report::from_error(&error).to_string();
    assert!(report.contains("unknown package system homebrew"));
}

#[test]
fn s3_publish_requests_are_limited_to_destination_branches() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let requests = genmeta_xtask_release::cli::parse_s3_publish_requests(
        &contract,
        &[os("deb"), os("rpm"), os("brew"), os("scoop")],
    )
    .expect("s3 publish requests should parse");

    assert_eq!(
        requests,
        [
            PackageSystem::Deb,
            PackageSystem::Rpm,
            PackageSystem::Brew,
            PackageSystem::Scoop,
        ]
    );
}

#[test]
fn s3_publish_requests_reject_system_without_destination_branch() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gateway.release.toml"))
            .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let error = genmeta_xtask_release::cli::parse_s3_publish_requests(&contract, &[os("scoop")])
        .expect_err("scoop should fail because gateway has no s3 scoop destination");

    assert_eq!(
        error.to_string(),
        "destination s3 scoop branch is not defined"
    );
}

#[test]
fn s3_publish_requests_reject_target_local_arguments() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let error = genmeta_xtask_release::cli::parse_s3_publish_requests(
        &contract,
        &[os("deb"), os("--target"), os("x86_64-unknown-linux-gnu")],
    )
    .expect_err("publish s3 target-local args should fail");

    assert_eq!(
        error.to_string(),
        "destination s3 deb branch does not accept target-local arguments"
    );
}

#[test]
fn s3_publish_requests_require_matching_package_system_branch() {
    let input = r#"
[package.sample]
manifest = "sample/Cargo.toml"

[package.sample.deb]
revision = "1"
architecture = "target"
dockerfile = "xtask/release/deb/Dockerfile"

[destination.s3]
bucket = "download"
endpoint.env = "XTASK_RELEASE_S3_ENDPOINT_URL"
access_key_id.env = "XTASK_RELEASE_S3_ACCESS_KEY_ID"
secret_access_key.env = "XTASK_RELEASE_S3_SECRET_ACCESS_KEY"

[destination.s3.scoop]
prefix = "scoop"
public_base_url = "https://download.dhttp.net/scoop"
"#;
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(input).expect("contract should parse");

    let error = genmeta_xtask_release::cli::parse_s3_publish_requests(&contract, &[os("scoop")])
        .expect_err("scoop publish should fail without a package scoop branch");

    assert_eq!(
        error.to_string(),
        "package system scoop is not defined by any package branch"
    );
}

#[test]
fn s3_publish_command_request_parses_dry_run_before_package_systems() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let request = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[os("--dry-run"), os("deb"), os("rpm"), os("scoop")],
    )
    .expect("s3 publish command request should parse");

    assert!(request.dry_run);
    assert_eq!(
        request.systems,
        [PackageSystem::Deb, PackageSystem::Rpm, PackageSystem::Scoop]
    );
}

#[test]
fn s3_publish_command_request_defaults_dry_run_to_false() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gateway.release.toml"))
            .expect("gateway fixture should parse");
    contract
        .validate()
        .expect("gateway fixture should validate");

    let request = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[os("deb"), os("rpm"), os("brew")],
    )
    .expect("s3 publish command request should parse");

    assert!(!request.dry_run);
    assert_eq!(
        request.systems,
        [PackageSystem::Deb, PackageSystem::Rpm, PackageSystem::Brew]
    );
}

#[test]
fn s3_publish_command_request_rejects_unknown_global_option() {
    let contract: genmeta_xtask_release::contract::ReleaseContract =
        toml::from_str(include_str!("fixtures/gmutils.release.toml"))
            .expect("gmutils fixture should parse");
    contract
        .validate()
        .expect("gmutils fixture should validate");

    let error = genmeta_xtask_release::cli::parse_s3_publish_command_request(
        &contract,
        &[os("--overwrite-manifest"), os("deb")],
    )
    .expect_err("unknown s3 publish command option should fail");

    let report = snafu::Report::from_error(&error).to_string();
    assert!(report.contains("unexpected argument '--overwrite-manifest'"));
}

#[test]
fn package_section_args_overlay_with_primary_mounts_primary_and_explicit_siblings() {
    let args = genmeta_xtask_release::cli::parse_package_section_args(&[
        os("--target"),
        os("x86_64-unknown-linux-gnu"),
        os("--sibling"),
        os("dhttp=/workspace/dhttp"),
        os("--patch"),
        os("crates-io"),
        os("dhttp"),
        os("dhttp/dhttp"),
    ])
    .expect("package section args should parse explicit sibling overlays");

    let overlay = args
        .container_overlay_plan_with_primary(genmeta_xtask_release::sibling::SiblingSource {
            name: "gmutils".to_string(),
            host_path: std::path::PathBuf::from("/workspace/gmutils"),
        })
        .expect("primary source overlay should plan")
        .expect("primary source overlay should exist");

    assert_eq!(
        overlay.mounts,
        [
            genmeta_xtask_release::sibling::ContainerMount {
                source: std::path::PathBuf::from("/workspace/gmutils"),
                destination: std::path::PathBuf::from("/sources/gmutils"),
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
}
