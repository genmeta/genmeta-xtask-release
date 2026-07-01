use std::{ffi::OsString, path::PathBuf, str::FromStr};

use snafu::{ResultExt, Snafu};

use crate::{
    contract::ReleaseContract,
    sibling::{
        ContainerOverlayPlan, ContainerOverlayPlanError, PatchOverride, PatchSource, SiblingSource,
    },
    system::{PackageSystem, RequestedTarget},
};

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
    let mut overwrite_manifest = false;
    let mut index = 0;
    while index < tokens.len() {
        let Some(option) = tokens[index].to_str() else {
            return Err(ParsePackageCommandRequestError::OptionNotUtf8);
        };
        if !option.starts_with('-') {
            break;
        }
        match option {
            "--overwrite-manifest" => {
                overwrite_manifest = true;
                index += 1;
            }
            other if other.starts_with('-') => {
                return Err(ParsePackageCommandRequestError::UnknownOption {
                    option: other.to_string(),
                });
            }
            other => {
                return Err(ParsePackageCommandRequestError::UnexpectedArgument {
                    argument: other.to_string(),
                });
            }
        }
    }

    let builds = parse_package_build_requests(contract, &tokens[index..])
        .context(parse_package_command_request_error::BuildRequestsSnafu)?;
    Ok(PackageCommandRequest {
        overwrite_manifest,
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
    let mut args = PackageSectionArgs {
        targets: Vec::new(),
        features: Vec::new(),
        siblings: Vec::new(),
        sibling_sources: Vec::new(),
        patches: Vec::new(),
        debug: false,
    };
    let mut index = 0;
    while index < tokens.len() {
        let Some(option) = tokens[index].to_str() else {
            return Err(ParsePackageSectionArgsError::OptionNotUtf8);
        };
        match option {
            "--target" => {
                let value = package_section_value(tokens, index, option)?;
                args.targets.push(parse_requested_target(value)?);
                index += 2;
            }
            "--features" => {
                let value = package_section_value(tokens, index, option)?;
                for feature in value.split(',') {
                    if feature.is_empty() {
                        return Err(ParsePackageSectionArgsError::EmptyFeature);
                    }
                    args.features.push(feature.to_string());
                }
                index += 2;
            }
            "--sibling" => {
                let value = package_section_os_value(tokens, index, option)?;
                args.siblings.push(PathBuf::from(value));
                if let Some(source) = parse_sibling_source(value)? {
                    args.sibling_sources.push(source);
                }
                index += 2;
            }
            "--patch" => {
                let source = package_section_value(tokens, index, option)?;
                let package = package_section_value(tokens, index + 1, option)?;
                let sibling_path = package_section_value(tokens, index + 2, option)?;
                args.patches
                    .push(parse_patch_override(source, package, sibling_path)?);
                index += 4;
            }
            "--debug" => {
                args.debug = true;
                index += 1;
            }
            other if other.starts_with('-') => {
                return Err(ParsePackageSectionArgsError::UnknownOption {
                    option: other.to_string(),
                });
            }
            other => {
                return Err(ParsePackageSectionArgsError::UnexpectedArgument {
                    argument: other.to_string(),
                });
            }
        }
    }

    if args.targets.is_empty() {
        return Err(ParsePackageSectionArgsError::MissingTarget);
    }
    Ok(args)
}

fn package_section_os_value<'a>(
    tokens: &'a [OsString],
    index: usize,
    option: &str,
) -> Result<&'a OsString, ParsePackageSectionArgsError> {
    tokens
        .get(index + 1)
        .ok_or_else(|| ParsePackageSectionArgsError::MissingOptionValue {
            option: option.to_string(),
        })
}

fn package_section_value<'a>(
    tokens: &'a [OsString],
    index: usize,
    option: &str,
) -> Result<&'a str, ParsePackageSectionArgsError> {
    let value =
        tokens
            .get(index + 1)
            .ok_or_else(|| ParsePackageSectionArgsError::MissingOptionValue {
                option: option.to_string(),
            })?;
    value
        .to_str()
        .ok_or(ParsePackageSectionArgsError::OptionValueNotUtf8 {
            option: option.to_string(),
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
    let mut dry_run = false;
    let mut index = 0;
    while index < tokens.len() {
        let Some(option) = tokens[index].to_str() else {
            return Err(ParseS3PublishCommandRequestError::OptionNotUtf8);
        };
        if !option.starts_with('-') {
            break;
        }
        match option {
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            other => {
                return Err(ParseS3PublishCommandRequestError::UnknownOption {
                    option: other.to_string(),
                });
            }
        }
    }

    let systems = parse_s3_publish_requests(contract, &tokens[index..])
        .context(parse_s3_publish_command_request_error::PublishRequestsSnafu)?;
    Ok(S3PublishCommandRequest { dry_run, systems })
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
