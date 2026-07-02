use std::{ffi::OsString, path::PathBuf, str::FromStr};

use clap::{ArgAction, Args, Parser, Subcommand};
use snafu::{ResultExt, Snafu};

use crate::{
    contract::ReleaseContract,
    sibling::{
        ContainerOverlayPlan, ContainerOverlayPlanError, PatchOverride, PatchSource, SiblingSource,
    },
    system::{PackageSystem, RequestedTarget},
};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Build & packaging tasks")]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: XtaskCommandCli,
}

#[derive(Debug, Subcommand)]
enum XtaskCommandCli {
    /// Build package artifacts and write package manifests
    Package(PackageCommandCli),
    /// Publish package manifests
    Publish(PublishCommandCli),
    /// Print derived release metadata for workflows
    Show(ShowCommandCli),
}

#[derive(Debug, Args)]
struct PackageCommandCli {
    /// Replace an existing manifest.toml
    #[arg(long)]
    overwrite_manifest: bool,
    /// Grouped package targets: deb/rpm/brew/scoop followed by target-local options
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    targets: Vec<OsString>,
}

#[derive(Debug, Parser)]
struct PackageCommandParser {
    #[command(flatten)]
    command: PackageCommandCli,
}

#[derive(Debug, Args)]
struct PackageSectionCli {
    /// Target triple or common
    #[arg(long = "target", required = true, value_parser = parse_requested_target)]
    targets: Vec<RequestedTarget>,
    /// Cargo features for this package-system section
    #[arg(long, value_delimiter = ',', value_parser = parse_feature)]
    features: Vec<String>,
    /// Sibling source path, or name=path
    #[arg(long = "sibling")]
    siblings: Vec<OsString>,
    /// Override a Cargo patch source: --patch <source> <package> <sibling/path>
    #[arg(long = "patch", num_args = 3, action = ArgAction::Append)]
    patches: Vec<String>,
    /// Build debug profile instead of release
    #[arg(long)]
    debug: bool,
}

#[derive(Debug, Parser)]
struct PackageSectionParser {
    #[command(flatten)]
    section: PackageSectionCli,
}

#[derive(Debug, Args)]
struct PublishCommandCli {
    #[command(subcommand)]
    command: PublishSubcommandCli,
}

#[derive(Debug, Subcommand)]
enum PublishSubcommandCli {
    /// Publish package manifests to S3/R2-compatible storage
    S3(S3PublishCommandCli),
}

#[derive(Debug, Args)]
struct S3PublishCommandCli {
    /// Show upload actions without mutating remote storage
    #[arg(long)]
    dry_run: bool,
    /// Package systems to publish
    #[arg(required = true, value_parser = parse_package_system)]
    systems: Vec<PackageSystem>,
}

#[derive(Debug, Parser)]
struct S3PublishCommandParser {
    #[command(flatten)]
    command: S3PublishCommandCli,
}

#[derive(Debug, Args)]
struct ShowCommandCli {
    #[command(subcommand)]
    command: ShowSubcommandCli,
}

#[derive(Debug, Subcommand)]
enum ShowSubcommandCli {
    /// Print the channel-selected S3 destination for a package system
    S3Destination(ShowS3DestinationCli),
}

#[derive(Debug, Args)]
struct ShowS3DestinationCli {
    #[arg(value_parser = parse_package_system)]
    system: PackageSystem,
    #[arg(long)]
    github_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XtaskCommandRequest {
    Package(PackageCommandRequest),
    Publish(PublishCommandRequest),
    Show(ShowCommandRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishCommandRequest {
    S3(S3PublishCommandRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowCommandRequest {
    S3Destination(ShowS3DestinationRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowS3DestinationRequest {
    pub system: PackageSystem,
    pub github_output: bool,
}

pub fn parse_xtask_command_request<I, T>(
    contract: &ReleaseContract,
    tokens: I,
) -> Result<XtaskCommandRequest, ParseXtaskCommandRequestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(tokens).context(parse_xtask_command_request_error::ClapSnafu)?;
    match cli.command {
        XtaskCommandCli::Package(command) => package_command_request(contract, command)
            .map(XtaskCommandRequest::Package)
            .context(parse_xtask_command_request_error::PackageSnafu),
        XtaskCommandCli::Publish(command) => publish_command_request(contract, command)
            .map(XtaskCommandRequest::Publish)
            .context(parse_xtask_command_request_error::PublishSnafu),
        XtaskCommandCli::Show(command) => show_command_request(contract, command)
            .map(XtaskCommandRequest::Show)
            .context(parse_xtask_command_request_error::ShowSnafu),
    }
}

pub fn parse_xtask_command_request_or_exit<I, T>(
    contract: &ReleaseContract,
    tokens: I,
) -> Result<XtaskCommandRequest, ParseXtaskCommandRequestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(tokens).unwrap_or_else(|error| error.exit());
    match cli.command {
        XtaskCommandCli::Package(command) => package_command_request(contract, command)
            .map(XtaskCommandRequest::Package)
            .context(parse_xtask_command_request_error::PackageSnafu),
        XtaskCommandCli::Publish(command) => publish_command_request(contract, command)
            .map(XtaskCommandRequest::Publish)
            .context(parse_xtask_command_request_error::PublishSnafu),
        XtaskCommandCli::Show(command) => show_command_request(contract, command)
            .map(XtaskCommandRequest::Show)
            .context(parse_xtask_command_request_error::ShowSnafu),
    }
}

fn publish_command_request(
    contract: &ReleaseContract,
    command: PublishCommandCli,
) -> Result<PublishCommandRequest, ParseS3PublishCommandRequestError> {
    match command.command {
        PublishSubcommandCli::S3(command) => {
            s3_publish_command_request(contract, command).map(PublishCommandRequest::S3)
        }
    }
}

fn show_command_request(
    contract: &ReleaseContract,
    command: ShowCommandCli,
) -> Result<ShowCommandRequest, ParseShowCommandRequestError> {
    match command.command {
        ShowSubcommandCli::S3Destination(command) => {
            if !destination_has_s3_package_system(contract, command.system) {
                return Err(ParseShowCommandRequestError::UndefinedDestinationBranch {
                    system: command.system,
                });
            }
            Ok(ShowCommandRequest::S3Destination(
                ShowS3DestinationRequest {
                    system: command.system,
                    github_output: command.github_output,
                },
            ))
        }
    }
}

fn parse_package_system(value: &str) -> Result<PackageSystem, String> {
    PackageSystem::from_str(value).map_err(|error| error.to_string())
}

fn parse_feature(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("feature must not be empty".to_string());
    }
    Ok(value.to_string())
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParseXtaskCommandRequestError {
    #[snafu(display("failed to parse command line"))]
    Clap { source: clap::Error },
    #[snafu(display("failed to parse package command"))]
    Package {
        source: ParsePackageCommandRequestError,
    },
    #[snafu(display("failed to parse publish command"))]
    Publish {
        source: ParseS3PublishCommandRequestError,
    },
    #[snafu(display("failed to parse show command"))]
    Show {
        source: ParseShowCommandRequestError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSystemSection {
    pub system: PackageSystem,
    pub args: Vec<OsString>,
}

pub fn parse_package_system_sections(
    tokens: &[OsString],
) -> Result<Vec<PackageSystemSection>, ParsePackageSystemSectionsError> {
    let mut sections = Vec::<PackageSystemSection>::new();

    for token in tokens {
        let Some(value) = token.to_str() else {
            let Some(section) = sections.last_mut() else {
                return Err(ParsePackageSystemSectionsError::PackageSystemNotUtf8);
            };
            section.args.push(token.clone());
            continue;
        };

        match PackageSystem::from_str(value) {
            Ok(system) => sections.push(PackageSystemSection {
                system,
                args: Vec::new(),
            }),
            Err(_) => {
                let Some(section) = sections.last_mut() else {
                    return if value.starts_with('-') {
                        Err(ParsePackageSystemSectionsError::ExpectedPackageSystem {
                            argument: value.to_owned(),
                        })
                    } else {
                        Err(ParsePackageSystemSectionsError::UnknownPackageSystem {
                            value: value.to_owned(),
                        })
                    };
                };
                section.args.push(token.clone());
            }
        }
    }

    if sections.is_empty() {
        return Err(ParsePackageSystemSectionsError::Empty);
    }
    Ok(sections)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBuildRequest {
    pub system: PackageSystem,
    pub args: PackageSectionArgs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCommandRequest {
    pub overwrite_manifest: bool,
    pub builds: Vec<PackageBuildRequest>,
}

pub fn parse_package_command_request(
    contract: &ReleaseContract,
    tokens: &[OsString],
) -> Result<PackageCommandRequest, ParsePackageCommandRequestError> {
    let command = PackageCommandParser::try_parse_from(
        std::iter::once(OsString::from("package")).chain(tokens.iter().cloned()),
    )
    .context(parse_package_command_request_error::ClapSnafu)?;
    package_command_request(contract, command.command)
}

fn package_command_request(
    contract: &ReleaseContract,
    command: PackageCommandCli,
) -> Result<PackageCommandRequest, ParsePackageCommandRequestError> {
    let builds = parse_package_build_requests(contract, &command.targets)
        .context(parse_package_command_request_error::BuildRequestsSnafu)?;
    Ok(PackageCommandRequest {
        overwrite_manifest: command.overwrite_manifest,
        builds,
    })
}

pub fn parse_package_build_requests(
    contract: &ReleaseContract,
    tokens: &[OsString],
) -> Result<Vec<PackageBuildRequest>, ParsePackageBuildRequestsError> {
    let sections = parse_package_system_sections(tokens)
        .context(parse_package_build_requests_error::PackageSystemsSnafu)?;
    let mut requests = Vec::with_capacity(sections.len());
    for section in sections {
        if !contract_has_package_system(contract, section.system) {
            return Err(ParsePackageBuildRequestsError::UndefinedPackageSystem {
                system: section.system,
            });
        }
        let args = parse_package_section_args(&section.args).context(
            parse_package_build_requests_error::SectionArgsSnafu {
                system: section.system,
            },
        )?;
        requests.push(PackageBuildRequest {
            system: section.system,
            args,
        });
    }
    Ok(requests)
}

fn contract_has_package_system(contract: &ReleaseContract, system: PackageSystem) -> bool {
    contract
        .package
        .values()
        .any(|package| package.branch(system).is_some())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSectionArgs {
    pub targets: Vec<RequestedTarget>,
    pub features: Vec<String>,
    pub siblings: Vec<PathBuf>,
    pub sibling_sources: Vec<SiblingSource>,
    pub patches: Vec<PatchOverride>,
    pub debug: bool,
}

impl PackageSectionArgs {
    pub fn container_overlay_plan(
        &self,
    ) -> Result<Option<ContainerOverlayPlan>, PackageSectionOverlayError> {
        if self.sibling_sources.is_empty() && self.patches.is_empty() {
            return Ok(None);
        }
        crate::sibling::container_overlay_plan(&self.sibling_sources, &self.patches)
            .map(Some)
            .context(package_section_overlay_error::ContainerSnafu)
    }

    pub fn container_overlay_plan_with_primary(
        &self,
        primary: SiblingSource,
    ) -> Result<Option<ContainerOverlayPlan>, PackageSectionOverlayError> {
        let mut sources = Vec::with_capacity(self.sibling_sources.len() + 1);
        sources.push(primary);
        sources.extend(self.sibling_sources.iter().cloned());
        crate::sibling::container_overlay_plan(&sources, &self.patches)
            .map(Some)
            .context(package_section_overlay_error::ContainerSnafu)
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PackageSectionOverlayError {
    #[snafu(display("failed to plan package section source overlay"))]
    Container { source: ContainerOverlayPlanError },
}

pub fn parse_package_section_args(
    tokens: &[OsString],
) -> Result<PackageSectionArgs, ParsePackageSectionArgsError> {
    let cli = PackageSectionParser::try_parse_from(
        std::iter::once(OsString::from("package-section")).chain(tokens.iter().cloned()),
    )
    .context(parse_package_section_args_error::ClapSnafu)?;
    let mut sibling_sources = Vec::new();
    for sibling in &cli.section.siblings {
        if let Some(source) = parse_sibling_source(sibling)? {
            sibling_sources.push(source);
        }
    }
    let mut patches = Vec::new();
    let (patch_args, remainder) = cli.section.patches.as_chunks::<3>();
    debug_assert!(remainder.is_empty());
    for [source, package, sibling_path] in patch_args {
        patches.push(parse_patch_override(source, package, sibling_path)?);
    }
    Ok(PackageSectionArgs {
        targets: cli.section.targets,
        features: cli.section.features,
        siblings: cli
            .section
            .siblings
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        sibling_sources,
        patches,
        debug: cli.section.debug,
    })
}

fn parse_sibling_source(
    value: &OsString,
) -> Result<Option<SiblingSource>, ParsePackageSectionArgsError> {
    let Some(value) = value.to_str() else {
        return Ok(None);
    };
    let Some((name, path)) = value.split_once('=') else {
        let path = PathBuf::from(value);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ParsePackageSectionArgsError::InvalidSiblingSource {
                value: value.to_string(),
            })?;
        return Ok(Some(SiblingSource {
            name: name.to_string(),
            host_path: path,
        }));
    };
    if name.is_empty() || path.is_empty() {
        return Err(ParsePackageSectionArgsError::InvalidSiblingSource {
            value: value.to_string(),
        });
    }
    Ok(Some(SiblingSource {
        name: name.to_string(),
        host_path: PathBuf::from(path),
    }))
}

fn parse_patch_override(
    source: &str,
    package: &str,
    sibling_path: &str,
) -> Result<PatchOverride, ParsePackageSectionArgsError> {
    if source.is_empty() || package.is_empty() || sibling_path.is_empty() {
        return Err(ParsePackageSectionArgsError::InvalidPatchOverride);
    }
    let (sibling, relative_path) =
        split_sibling_path(sibling_path).ok_or(ParsePackageSectionArgsError::InvalidPatchPath)?;
    Ok(PatchOverride {
        source: if source == "crates-io" {
            PatchSource::CratesIo
        } else {
            PatchSource::Git(source.to_string())
        },
        package: package.to_string(),
        sibling,
        relative_path,
    })
}

fn split_sibling_path(value: &str) -> Option<(String, PathBuf)> {
    let (sibling, relative) = value.split_once('/').unwrap_or((value, "."));
    if sibling.is_empty() || relative.is_empty() {
        return None;
    }
    Some((sibling.to_string(), PathBuf::from(relative)))
}

fn parse_requested_target(value: &str) -> Result<RequestedTarget, ParsePackageSectionArgsError> {
    if value.is_empty() {
        return Err(ParsePackageSectionArgsError::EmptyTarget);
    }
    if value == "common" {
        return Ok(RequestedTarget::Common);
    }
    Ok(RequestedTarget::Triple(value.to_string()))
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParsePackageSystemSectionsError {
    #[snafu(display("at least one package system is required"))]
    Empty,
    #[snafu(display("package system name must be utf-8"))]
    PackageSystemNotUtf8,
    #[snafu(display("expected a package system before argument {argument}"))]
    ExpectedPackageSystem { argument: String },
    #[snafu(display("unknown package system {value}"))]
    UnknownPackageSystem { value: String },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParsePackageSectionArgsError {
    #[snafu(display("failed to parse package section arguments"))]
    Clap { source: clap::Error },
    #[snafu(display("package section option must be utf-8"))]
    OptionNotUtf8,
    #[snafu(display("package section option {option} requires a value"))]
    MissingOptionValue { option: String },
    #[snafu(display("package section option {option} value must be utf-8"))]
    OptionValueNotUtf8 { option: String },
    #[snafu(display("package section requires at least one target"))]
    MissingTarget,
    #[snafu(display("package section target must not be empty"))]
    EmptyTarget,
    #[snafu(display("package section feature must not be empty"))]
    EmptyFeature,
    #[snafu(display("package section sibling source must be name=path"))]
    InvalidSiblingSource { value: String },
    #[snafu(display("package section patch override is invalid"))]
    InvalidPatchOverride,
    #[snafu(display("package section patch path must identify a sibling source"))]
    InvalidPatchPath,
    #[snafu(display("unknown package section option {option}"))]
    UnknownOption { option: String },
    #[snafu(display("unexpected package section argument {argument}"))]
    UnexpectedArgument { argument: String },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParsePackageCommandRequestError {
    #[snafu(display("failed to parse package command arguments"))]
    Clap { source: clap::Error },
    #[snafu(display("package command option must be utf-8"))]
    OptionNotUtf8,
    #[snafu(display("unknown package command option {option}"))]
    UnknownOption { option: String },
    #[snafu(display("unexpected package command argument {argument}"))]
    UnexpectedArgument { argument: String },
    #[snafu(display("failed to parse package build requests"))]
    BuildRequests {
        source: ParsePackageBuildRequestsError,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParsePackageBuildRequestsError {
    #[snafu(display("failed to parse package systems"))]
    PackageSystems {
        source: ParsePackageSystemSectionsError,
    },
    #[snafu(display("package system {system} is not defined by any package branch"))]
    UndefinedPackageSystem { system: PackageSystem },
    #[snafu(display("failed to parse {system} package section"))]
    SectionArgs {
        source: ParsePackageSectionArgsError,
        system: PackageSystem,
    },
}

pub fn parse_s3_publish_requests(
    contract: &ReleaseContract,
    tokens: &[OsString],
) -> Result<Vec<PackageSystem>, ParseS3PublishRequestsError> {
    let sections = parse_package_system_sections(tokens)
        .context(parse_s3_publish_requests_error::PackageSystemsSnafu)?;
    let mut requests = Vec::with_capacity(sections.len());
    for section in sections {
        if !section.args.is_empty() {
            return Err(ParseS3PublishRequestsError::TargetLocalArguments {
                system: section.system,
            });
        }
        if !destination_has_s3_package_system(contract, section.system) {
            return Err(ParseS3PublishRequestsError::UndefinedDestinationBranch {
                system: section.system,
            });
        }
        if !contract_has_package_system(contract, section.system) {
            return Err(ParseS3PublishRequestsError::UndefinedPackageSystem {
                system: section.system,
            });
        }
        requests.push(section.system);
    }
    Ok(requests)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3PublishCommandRequest {
    pub dry_run: bool,
    pub systems: Vec<PackageSystem>,
}

pub fn parse_s3_publish_command_request(
    contract: &ReleaseContract,
    tokens: &[OsString],
) -> Result<S3PublishCommandRequest, ParseS3PublishCommandRequestError> {
    let command = S3PublishCommandParser::try_parse_from(
        std::iter::once(OsString::from("s3")).chain(tokens.iter().cloned()),
    )
    .context(parse_s3_publish_command_request_error::ClapSnafu)?;
    s3_publish_command_request(contract, command.command)
}

fn s3_publish_command_request(
    contract: &ReleaseContract,
    command: S3PublishCommandCli,
) -> Result<S3PublishCommandRequest, ParseS3PublishCommandRequestError> {
    validate_s3_publish_systems(contract, &command.systems)
        .context(parse_s3_publish_command_request_error::PublishRequestsSnafu)?;
    Ok(S3PublishCommandRequest {
        dry_run: command.dry_run,
        systems: command.systems,
    })
}

fn validate_s3_publish_systems(
    contract: &ReleaseContract,
    systems: &[PackageSystem],
) -> Result<(), ParseS3PublishRequestsError> {
    for &system in systems {
        if !destination_has_s3_package_system(contract, system) {
            return Err(ParseS3PublishRequestsError::UndefinedDestinationBranch { system });
        }
        if !contract_has_package_system(contract, system) {
            return Err(ParseS3PublishRequestsError::UndefinedPackageSystem { system });
        }
    }
    Ok(())
}

fn destination_has_s3_package_system(contract: &ReleaseContract, system: PackageSystem) -> bool {
    match system {
        PackageSystem::Deb => contract.destination.s3.deb.is_some(),
        PackageSystem::Rpm => contract.destination.s3.rpm.is_some(),
        PackageSystem::Brew => contract.destination.s3.brew.is_some(),
        PackageSystem::Scoop => contract.destination.s3.scoop.is_some(),
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParseS3PublishCommandRequestError {
    #[snafu(display("failed to parse s3 publish command arguments"))]
    Clap { source: clap::Error },
    #[snafu(display("s3 publish command option must be utf-8"))]
    OptionNotUtf8,
    #[snafu(display("unknown s3 publish command option {option}"))]
    UnknownOption { option: String },
    #[snafu(display("failed to parse s3 publish requests"))]
    PublishRequests { source: ParseS3PublishRequestsError },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParseS3PublishRequestsError {
    #[snafu(display("failed to parse package systems"))]
    PackageSystems {
        source: ParsePackageSystemSectionsError,
    },
    #[snafu(display("destination s3 {system} branch is not defined"))]
    UndefinedDestinationBranch { system: PackageSystem },
    #[snafu(display("destination s3 {system} branch does not accept target-local arguments"))]
    TargetLocalArguments { system: PackageSystem },
    #[snafu(display("package system {system} is not defined by any package branch"))]
    UndefinedPackageSystem { system: PackageSystem },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ParseShowCommandRequestError {
    #[snafu(display("destination s3 {system} branch is missing"))]
    UndefinedDestinationBranch { system: PackageSystem },
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::Parser;

    use super::{
        parse_package_command_request, parse_s3_publish_command_request,
        parse_xtask_command_request,
    };
    use crate::{contract::ReleaseContract, system::RequestedTarget};

    fn contract() -> ReleaseContract {
        let contract: ReleaseContract = toml::from_str(
            r#"
                [package.product]
                version = "1.2.3"
                description = "test product"
                license = "Apache-2.0"
                homepage = "https://example.test"

                [package.product.rpm]
                release = "1"
                architecture = "target"
                dockerfile = "xtask/release/rpm/Dockerfile"

                [package.product.deb]
                revision = "1"
                architecture = "target"
                dockerfile = "xtask/release/deb/Dockerfile"

                [package.product.brew]
                script = "xtask/release/brew/product.sh"
                manifest_template = "xtask/templates/product.rb.in"

                [destination.s3]
                bucket = "release"
                endpoint.env = "S3_ENDPOINT"
                access_key_id.env = "S3_ACCESS_KEY_ID"
                secret_access_key.env = "S3_SECRET_ACCESS_KEY"

                [destination.s3.rpm.stable]
                prefix = "rpm/stable"

                [destination.s3.rpm.preview]
                prefix = "rpm/preview"

                [destination.s3.deb.stable]
                prefix = "deb/product"
                suite = "stable"

                [destination.s3.deb.preview]
                prefix = "deb/product"
                suite = "preview"

                [destination.s3.deb.signing]
                key.env = "APT_SIGNING_KEY"
                passphrase.env = "APT_SIGNING_PASSPHRASE"
                fingerprint.env = "APT_SIGNING_FINGERPRINT"

                [destination.s3.brew.stable]
                prefix = "homebrew/stable"
                public_base_url = "https://download.dhttp.net/homebrew/stable"
                tap.repository = "genmeta/homebrew-stable"
                tap.base_branch = "main"
                tap.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"

                [destination.s3.brew.preview]
                prefix = "homebrew/preview"
                public_base_url = "https://download.dhttp.net/homebrew/preview"
                tap.repository = "genmeta/homebrew-preview"
                tap.base_branch = "main"
                tap.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"
            "#,
        )
        .expect("fixture contract should parse");
        contract
            .validate()
            .expect("fixture contract should validate");
        contract
    }

    fn os_args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn top_level_help_is_generated_by_clap() {
        let error =
            super::Cli::try_parse_from(["xtask", "--help"]).expect_err("help should exit early");

        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("Build & packaging tasks"));
    }

    #[test]
    fn package_command_uses_clap_for_grouped_package_sections() {
        let command = parse_package_command_request(
            &contract(),
            &os_args(&[
                "--overwrite-manifest",
                "rpm",
                "--target",
                "common",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--features",
                "sshd,pam",
                "--sibling",
                "dhttp=../dhttp",
                "--patch",
                "crates-io",
                "dhttp",
                "dhttp/dhttp",
            ]),
        )
        .expect("package command should parse");

        assert!(command.overwrite_manifest);
        assert_eq!(command.builds.len(), 1);
        let build = &command.builds[0];
        assert_eq!(
            build.args.targets,
            vec![
                RequestedTarget::Common,
                RequestedTarget::Triple("x86_64-unknown-linux-gnu".to_string())
            ]
        );
        assert_eq!(build.args.features, vec!["sshd", "pam"]);
        assert_eq!(build.args.sibling_sources[0].name, "dhttp");
        assert_eq!(build.args.patches[0].package, "dhttp");
    }

    #[test]
    fn package_section_rejects_unknown_options_with_clap() {
        let error = parse_package_command_request(
            &contract(),
            &os_args(&["rpm", "--target", "x86_64-unknown-linux-gnu", "--unknown"]),
        )
        .expect_err("unknown package section option should fail");

        let report = snafu::Report::from_error(&error).to_string();
        assert!(report.contains("unexpected argument '--unknown'"));
    }

    #[test]
    fn s3_publish_command_uses_clap_for_flags_and_systems() {
        let command =
            parse_s3_publish_command_request(&contract(), &os_args(&["--dry-run", "deb", "rpm"]))
                .expect("s3 publish command should parse");

        assert!(command.dry_run);
        assert_eq!(command.systems.len(), 2);
    }

    #[test]
    fn show_s3_destination_parses_system_and_github_output_flag() {
        let command = parse_xtask_command_request(
            &contract(),
            os_args(&["xtask", "show", "s3-destination", "brew", "--github-output"]),
        )
        .expect("show command should parse");

        let super::XtaskCommandRequest::Show(super::ShowCommandRequest::S3Destination(command)) =
            command
        else {
            panic!("expected show s3-destination command");
        };
        assert_eq!(command.system, crate::system::PackageSystem::Brew);
        assert!(command.github_output);
    }
}
