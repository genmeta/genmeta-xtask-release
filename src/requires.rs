use std::{collections::BTreeMap, path::Path};

use semver::Version;
use snafu::{ResultExt, Snafu};

use crate::{
    contract::{PackageBranchRef, ReleaseContract, VersionBoundSource, VersionBoundSourceContract},
    package::{PackageVersion, resolve_metadata},
    system::PackageSystem,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVersionBounds {
    pub minimum: Option<String>,
    pub maximum: Option<String>,
}

pub fn linux_requirement_entries(
    system: PackageSystem,
    package: &str,
    bounds: ResolvedVersionBounds,
) -> Result<Vec<String>, RenderLinuxRequirementError> {
    match system {
        PackageSystem::Deb => Ok(version_bound_entries(
            package,
            bounds,
            |package, version| format!("{package} (>= {version})"),
            |package, version| format!("{package} (<= {version})"),
        )),
        PackageSystem::Rpm => Ok(version_bound_entries(
            package,
            bounds,
            |package, version| format!("{package} >= {version}"),
            |package, version| format!("{package} <= {version}"),
        )),
        PackageSystem::Brew | PackageSystem::Scoop => {
            Err(RenderLinuxRequirementError::UnsupportedPackageSystem { system })
        }
    }
}

fn version_bound_entries(
    package: &str,
    bounds: ResolvedVersionBounds,
    minimum_entry: impl FnOnce(&str, &str) -> String,
    maximum_entry: impl FnOnce(&str, &str) -> String,
) -> Vec<String> {
    let mut entries = Vec::new();
    if let Some(version) = bounds.minimum {
        entries.push(minimum_entry(package, &version));
    }
    if let Some(version) = bounds.maximum {
        entries.push(maximum_entry(package, &version));
    }
    if entries.is_empty() {
        entries.push(package.to_owned());
    }
    entries
}

pub fn resolve_requires_for(
    contract: &ReleaseContract,
    contract_root: &Path,
    package: &str,
    system: PackageSystem,
) -> Result<BTreeMap<String, ResolvedVersionBounds>, ResolveRequiresError> {
    let (_, package_contract) =
        contract
            .package_entry(package)
            .ok_or_else(|| ResolveRequiresError::MissingPackage {
                package: package.to_owned(),
            })?;
    let self_source_version = resolve_metadata(contract, package, contract_root)
        .context(resolve_requires_error::PackageMetadataSnafu {
            package: package.to_owned(),
        })?
        .source_version;
    let branch =
        package_contract
            .branch(system)
            .ok_or_else(|| ResolveRequiresError::MissingBranch {
                package: package.to_owned(),
                system,
            })?;
    let self_version = package_version_for_branch(branch, self_source_version)?;

    let mut resolved = BTreeMap::new();
    for (dependency_id, required) in branch.requires().iter() {
        let dependency_version =
            dependency_package_version(contract, contract_root, dependency_id.as_str(), system)?;
        let minimum = required
            .version
            .minimum
            .as_ref()
            .map(|bound| resolve_version_bound(bound, &self_version, &dependency_version));
        let maximum = required
            .version
            .maximum
            .as_ref()
            .map(|bound| resolve_version_bound(bound, &self_version, &dependency_version));
        resolved.insert(
            dependency_id.as_str().to_owned(),
            ResolvedVersionBounds { minimum, maximum },
        );
    }
    Ok(resolved)
}

fn resolve_version_bound(
    bound: &VersionBoundSourceContract,
    self_version: &PackageVersion,
    dependency_version: &PackageVersion,
) -> String {
    match bound {
        VersionBoundSourceContract::Source(VersionBoundSource::SelfPackage) => {
            self_version.as_string()
        }
        VersionBoundSourceContract::Source(VersionBoundSource::DependencyPackage) => {
            dependency_version.as_string()
        }
        VersionBoundSourceContract::Literal(value) => value.clone(),
    }
}

fn dependency_package_version(
    contract: &ReleaseContract,
    contract_root: &Path,
    package: &str,
    system: PackageSystem,
) -> Result<PackageVersion, ResolveRequiresError> {
    let (_, package_contract) =
        contract
            .package_entry(package)
            .ok_or_else(|| ResolveRequiresError::MissingPackage {
                package: package.to_owned(),
            })?;
    let source = resolve_metadata(contract, package, contract_root)
        .context(resolve_requires_error::DependencyMetadataSnafu {
            package: package.to_owned(),
        })?
        .source_version;
    let branch =
        package_contract
            .branch(system)
            .ok_or_else(|| ResolveRequiresError::MissingBranch {
                package: package.to_owned(),
                system,
            })?;
    package_version_for_branch(branch, source)
}

fn package_version_for_branch(
    branch: PackageBranchRef<'_>,
    source: Version,
) -> Result<PackageVersion, ResolveRequiresError> {
    match branch {
        PackageBranchRef::Deb(branch) => PackageVersion::deb(source, branch.revision.clone())
            .context(resolve_requires_error::InvalidPackageVersionSnafu),
        PackageBranchRef::Rpm(branch) => PackageVersion::rpm(source, branch.release.clone())
            .context(resolve_requires_error::InvalidPackageVersionSnafu),
        PackageBranchRef::Brew(_) | PackageBranchRef::Scoop(_) => Ok(PackageVersion::plain(source)),
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RenderLinuxRequirementError {
    #[snafu(display("{system} branch does not support linux dependency entries"))]
    UnsupportedPackageSystem { system: PackageSystem },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ResolveRequiresError {
    #[snafu(display("package {package} does not exist"))]
    MissingPackage { package: String },
    #[snafu(display("package {package} does not define {system} branch"))]
    MissingBranch {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("failed to resolve package {package} metadata"))]
    PackageMetadata {
        package: String,
        source: crate::package::ResolvePackageMetadataError,
    },
    #[snafu(display("failed to resolve package {package} dependency metadata"))]
    DependencyMetadata {
        package: String,
        source: crate::package::ResolvePackageMetadataError,
    },
    #[snafu(display("failed to compose package version"))]
    InvalidPackageVersion {
        source: crate::package::PackageVersionError,
    },
}
