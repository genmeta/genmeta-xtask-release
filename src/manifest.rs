use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::Digest;
use snafu::{ResultExt, Snafu};

use crate::{
    cli::{PackageCommandRequest, S3PublishCommandRequest},
    contract::{PackageBranchRef, ReleaseContract},
    package::PackageVersion,
    plan::{PackageSelectionRequest, SelectPackageError, select_package_branches},
    system::{PackageSystem, RequestedTarget},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PackageManifest {
    pub schema_version: u32,
    pub kind: PackageSystem,
    pub package: String,
    pub version: String,
    pub generated_at: String,
    pub git_commit: Option<String>,
    pub git_dirty: bool,
    pub artifacts: Vec<PackageArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct PackageArtifact {
    pub target: String,
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

pub fn manifest_path(target_dir: &Path, system: PackageSystem) -> PathBuf {
    target_dir
        .join("common")
        .join(system.as_str())
        .join("manifest.toml")
}

pub fn load_manifest(
    target_dir: &Path,
    system: PackageSystem,
) -> Result<PackageManifest, LoadManifestError> {
    let path = manifest_path(target_dir, system);
    let content = fs::read_to_string(&path)
        .context(load_manifest_error::ReadManifestSnafu { path: path.clone() })?;
    let manifest: PackageManifest = toml::from_str(&content)
        .context(load_manifest_error::ParseManifestSnafu { path: path.clone() })?;
    validate_manifest(&manifest).context(load_manifest_error::InvalidManifestSnafu)?;
    Ok(manifest)
}

pub fn load_s3_publish_command_manifests(
    target_dir: &Path,
    contract: &ReleaseContract,
    command: &S3PublishCommandRequest,
) -> Result<Vec<PackageManifest>, LoadS3PublishCommandManifestsError> {
    let mut manifests = Vec::with_capacity(command.systems.len());
    for system in &command.systems {
        let manifest = load_manifest(target_dir, *system)
            .context(load_s3_publish_command_manifests_error::LoadSnafu { system: *system })?;
        validate_manifest_against_contract(&manifest, contract)
            .context(load_s3_publish_command_manifests_error::ContractSnafu { system: *system })?;
        verify_manifest_artifacts(target_dir, &manifest)
            .context(load_s3_publish_command_manifests_error::VerifySnafu { system: *system })?;
        manifests.push(manifest);
    }
    Ok(manifests)
}

pub fn write_manifest(
    target_dir: &Path,
    manifest: &PackageManifest,
    overwrite: bool,
) -> Result<(), WriteManifestError> {
    validate_manifest(manifest).context(write_manifest_error::InvalidManifestSnafu)?;
    let path = manifest_path(target_dir, manifest.kind);
    if path.exists() && !overwrite {
        return Err(WriteManifestError::AlreadyExists { path });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context(write_manifest_error::CreateManifestDirectorySnafu {
            path: parent.to_path_buf(),
        })?;
    }
    let content =
        toml::to_string_pretty(manifest).context(write_manifest_error::SerializeManifestSnafu)?;
    let mut file = fs::File::create(&path)
        .context(write_manifest_error::CreateManifestSnafu { path: path.clone() })?;
    file.write_all(content.as_bytes())
        .context(write_manifest_error::WriteManifestSnafu { path })?;
    Ok(())
}

pub fn write_package_command_manifest(
    target_dir: &Path,
    manifest: &PackageManifest,
    contract: &ReleaseContract,
    command: &PackageCommandRequest,
) -> Result<(), WritePackageCommandManifestError> {
    validate_package_command_manifest(manifest, contract, command)
        .context(write_package_command_manifest_error::ValidateSnafu)?;
    write_manifest(target_dir, manifest, command.overwrite_manifest)
        .context(write_package_command_manifest_error::WriteSnafu)?;
    Ok(())
}

pub fn validate_manifest(manifest: &PackageManifest) -> Result<(), ValidateManifestError> {
    let mut linux_keys = BTreeSet::new();
    for artifact in &manifest.artifacts {
        validate_target_relative_path(&artifact.path)?;
        if matches!(manifest.kind, PackageSystem::Deb | PackageSystem::Rpm) {
            let package = artifact
                .package_name
                .clone()
                .ok_or(ValidateManifestError::MissingPackageName)?;
            artifact
                .package_version
                .as_ref()
                .ok_or(ValidateManifestError::MissingPackageVersion)?;
            let architecture = artifact
                .architecture
                .clone()
                .ok_or(ValidateManifestError::MissingArchitecture)?;
            if !linux_keys.insert((package.clone(), architecture.clone())) {
                return Err(ValidateManifestError::DuplicatePackageArchitecture {
                    package,
                    architecture,
                });
            }
        }
        if matches!(manifest.kind, PackageSystem::Brew | PackageSystem::Scoop) {
            artifact
                .archive_name
                .as_ref()
                .ok_or(ValidateManifestError::MissingArchiveName)?;
        }
    }
    Ok(())
}

pub fn validate_manifest_against_contract(
    manifest: &PackageManifest,
    contract: &ReleaseContract,
) -> Result<(), ValidateManifestAgainstContractError> {
    validate_manifest(manifest).context(validate_manifest_against_contract_error::ManifestSnafu)?;
    let package = contract.package(&manifest.package).ok_or_else(|| {
        ValidateManifestAgainstContractError::MissingPackage {
            package: manifest.package.clone(),
        }
    })?;
    package.branch(manifest.kind).ok_or_else(|| {
        ValidateManifestAgainstContractError::MissingBranch {
            package: manifest.package.clone(),
            system: manifest.kind,
        }
    })?;
    if matches!(manifest.kind, PackageSystem::Deb | PackageSystem::Rpm) {
        for artifact in &manifest.artifacts {
            let package = artifact
                .package_name
                .as_ref()
                .ok_or(ValidateManifestAgainstContractError::MissingArtifactPackageName)?;
            let package_contract = contract.package(package).ok_or_else(|| {
                ValidateManifestAgainstContractError::MissingArtifactPackage {
                    package: package.clone(),
                }
            })?;
            let branch = package_contract.branch(manifest.kind).ok_or_else(|| {
                ValidateManifestAgainstContractError::MissingArtifactBranch {
                    package: package.clone(),
                    system: manifest.kind,
                }
            })?;
            if let Some(expected) =
                expected_artifact_package_version(package, manifest, package_contract, branch)?
            {
                let actual = artifact.package_version.as_ref().ok_or(
                    ValidateManifestAgainstContractError::MissingArtifactPackageVersion {
                        package: package.clone(),
                    },
                )?;
                if actual != &expected {
                    return Err(
                        ValidateManifestAgainstContractError::PackageVersionMismatch {
                            package: package.clone(),
                            system: manifest.kind,
                            actual: actual.clone(),
                            expected,
                        },
                    );
                }
            }
        }
    }

    Ok(())
}

fn expected_artifact_package_version(
    package: &str,
    manifest: &PackageManifest,
    package_contract: &crate::contract::PackageContract,
    branch: PackageBranchRef<'_>,
) -> Result<Option<String>, ValidateManifestAgainstContractError> {
    let version = if let Some(version) = &package_contract.version {
        version
    } else if package == manifest.package {
        &manifest.version
    } else {
        return Ok(None);
    };
    let source = semver::Version::parse(version).context(
        validate_manifest_against_contract_error::InvalidPackageSourceVersionSnafu {
            package: package.to_string(),
            version: version.clone(),
        },
    )?;
    let package_version = match branch {
        PackageBranchRef::Deb(branch) => PackageVersion::deb(source, branch.revision.clone()),
        PackageBranchRef::Rpm(branch) => PackageVersion::rpm(source, branch.release.clone()),
        PackageBranchRef::Brew(_) | PackageBranchRef::Scoop(_) => Ok(PackageVersion::plain(source)),
    }
    .context(
        validate_manifest_against_contract_error::InvalidExpectedPackageVersionSnafu {
            package: package.to_string(),
            system: manifest.kind,
        },
    )?;
    Ok(Some(package_version.as_string()))
}

pub fn validate_manifest_targets(
    manifest: &PackageManifest,
    requested_targets: &[RequestedTarget],
) -> Result<(), ValidateManifestTargetsError> {
    validate_manifest(manifest).context(validate_manifest_targets_error::ManifestSnafu)?;
    let manifest_targets = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.target.clone())
        .collect::<BTreeSet<_>>();
    let requested_targets = requested_targets
        .iter()
        .map(requested_target_manifest_value)
        .collect::<BTreeSet<_>>();
    for target in &manifest_targets {
        if !requested_targets.contains(target) {
            return Err(ValidateManifestTargetsError::UnrequestedTarget {
                system: manifest.kind,
                target: target.clone(),
            });
        }
    }
    for target in requested_targets {
        if !manifest_targets.contains(&target) {
            return Err(ValidateManifestTargetsError::MissingRequestedTarget {
                system: manifest.kind,
                target,
            });
        }
    }

    Ok(())
}

pub fn validate_package_command_manifest(
    manifest: &PackageManifest,
    contract: &ReleaseContract,
    command: &PackageCommandRequest,
) -> Result<(), ValidatePackageCommandManifestError> {
    validate_manifest_against_contract(manifest, contract)
        .context(validate_package_command_manifest_error::ContractSnafu)?;
    let mut requested_targets = Vec::new();
    for build in &command.builds {
        if build.system != manifest.kind {
            continue;
        }
        let selected = select_package_branches(
            contract,
            PackageSelectionRequest {
                system: build.system,
                targets: build.args.targets.clone(),
                features: build.args.features.clone(),
            },
        )
        .context(validate_package_command_manifest_error::SelectSnafu {
            system: build.system,
        })?;
        for selected_build in selected {
            if selected_build.package_id.as_str() == manifest.package {
                requested_targets.push(selected_build.target);
            }
        }
    }
    if requested_targets.is_empty() {
        return Err(ValidatePackageCommandManifestError::MissingSystem {
            system: manifest.kind,
        });
    }
    validate_manifest_targets(manifest, &requested_targets)
        .context(validate_package_command_manifest_error::TargetsSnafu)?;
    Ok(())
}

fn requested_target_manifest_value(target: &RequestedTarget) -> String {
    match target {
        RequestedTarget::Triple(triple) => triple.clone(),
        RequestedTarget::Common => "common".to_string(),
    }
}

pub fn verify_manifest_artifacts(
    target_dir: &Path,
    manifest: &PackageManifest,
) -> Result<(), VerifyManifestArtifactError> {
    validate_manifest(manifest).context(verify_manifest_artifact_error::InvalidManifestSnafu)?;
    for artifact in &manifest.artifacts {
        let path = target_dir.join(&artifact.path);
        let metadata = fs::metadata(&path)
            .context(verify_manifest_artifact_error::MetadataSnafu { path: path.clone() })?;
        if metadata.len() != artifact.size {
            return Err(VerifyManifestArtifactError::SizeMismatch {
                path,
                expected: artifact.size,
                actual: metadata.len(),
            });
        }
        let sha256 = file_sha256(&path)?;
        if sha256 != artifact.sha256 {
            return Err(VerifyManifestArtifactError::Sha256Mismatch {
                path,
                expected: artifact.sha256.clone(),
                actual: sha256,
            });
        }
    }
    Ok(())
}

fn validate_target_relative_path(value: &str) -> Result<(), ValidateManifestError> {
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(ValidateManifestError::AbsolutePath);
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ValidateManifestError::ParentComponent);
    }
    Ok(())
}

fn file_sha256(path: &Path) -> Result<String, VerifyManifestArtifactError> {
    let mut file = fs::File::open(path).context(verify_manifest_artifact_error::OpenSnafu {
        path: path.to_path_buf(),
    })?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .context(verify_manifest_artifact_error::ReadSnafu {
                path: path.to_path_buf(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LoadManifestError {
    #[snafu(display("failed to read package manifest"))]
    ReadManifest {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to parse package manifest"))]
    ParseManifest {
        source: toml::de::Error,
        path: PathBuf,
    },
    #[snafu(display("invalid package manifest"))]
    InvalidManifest { source: ValidateManifestError },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LoadS3PublishCommandManifestsError {
    #[snafu(display("failed to load {system} package manifest"))]
    Load {
        source: LoadManifestError,
        system: PackageSystem,
    },
    #[snafu(display("failed to validate {system} package manifest against release contract"))]
    Contract {
        source: ValidateManifestAgainstContractError,
        system: PackageSystem,
    },
    #[snafu(display("failed to verify {system} package artifacts"))]
    Verify {
        source: VerifyManifestArtifactError,
        system: PackageSystem,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum WriteManifestError {
    #[snafu(display("invalid package manifest"))]
    InvalidManifest { source: ValidateManifestError },
    #[snafu(display("package manifest already exists at {path:?}"))]
    AlreadyExists { path: PathBuf },
    #[snafu(display("failed to create package manifest directory"))]
    CreateManifestDirectory {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to serialize package manifest"))]
    SerializeManifest { source: toml::ser::Error },
    #[snafu(display("failed to create package manifest"))]
    CreateManifest {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to write package manifest"))]
    WriteManifest {
        source: std::io::Error,
        path: PathBuf,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum WritePackageCommandManifestError {
    #[snafu(display("failed to validate package command manifest"))]
    Validate {
        source: ValidatePackageCommandManifestError,
    },
    #[snafu(display("failed to write package command manifest"))]
    Write { source: WriteManifestError },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ValidateManifestError {
    #[snafu(display("artifact path must be target-relative"))]
    AbsolutePath,
    #[snafu(display("artifact path must not contain parent components"))]
    ParentComponent,
    #[snafu(display("linux package artifact must include package name"))]
    MissingPackageName,
    #[snafu(display("linux package artifact must include package version"))]
    MissingPackageVersion,
    #[snafu(display("linux package artifact must include architecture"))]
    MissingArchitecture,
    #[snafu(display("package artifact must include archive name"))]
    MissingArchiveName,
    #[snafu(display("duplicate package artifact for {package} {architecture}"))]
    DuplicatePackageArchitecture {
        package: String,
        architecture: String,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ValidateManifestAgainstContractError {
    #[snafu(display("invalid package manifest"))]
    Manifest { source: ValidateManifestError },
    #[snafu(display("package {package} does not exist"))]
    MissingPackage { package: String },
    #[snafu(display("package {package} does not define {system} branch"))]
    MissingBranch {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("linux package artifact is missing package name"))]
    MissingArtifactPackageName,
    #[snafu(display("linux package artifact {package} does not exist"))]
    MissingArtifactPackage { package: String },
    #[snafu(display("linux package artifact {package} does not define {system} branch"))]
    MissingArtifactBranch {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("linux package artifact {package} is missing package version"))]
    MissingArtifactPackageVersion { package: String },
    #[snafu(display("package {package} has invalid source version {version}"))]
    InvalidPackageSourceVersion {
        source: semver::Error,
        package: String,
        version: String,
    },
    #[snafu(display("package {package} {system} branch has invalid package version"))]
    InvalidExpectedPackageVersion {
        source: crate::package::PackageVersionError,
        package: String,
        system: PackageSystem,
    },
    #[snafu(display(
        "linux package artifact {package} {system} version {actual} does not match expected {expected}"
    ))]
    PackageVersionMismatch {
        package: String,
        system: PackageSystem,
        actual: String,
        expected: String,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ValidateManifestTargetsError {
    #[snafu(display("invalid package manifest"))]
    Manifest { source: ValidateManifestError },
    #[snafu(display("{system} package manifest is missing requested target {target}"))]
    MissingRequestedTarget {
        system: PackageSystem,
        target: String,
    },
    #[snafu(display("{system} package manifest contains unrequested target {target}"))]
    UnrequestedTarget {
        system: PackageSystem,
        target: String,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ValidatePackageCommandManifestError {
    #[snafu(display("failed to validate package manifest against release contract"))]
    Contract {
        source: ValidateManifestAgainstContractError,
    },
    #[snafu(display("package command did not request {system} build"))]
    MissingSystem { system: PackageSystem },
    #[snafu(display("failed to select package builds for {system}"))]
    Select {
        source: SelectPackageError,
        system: PackageSystem,
    },
    #[snafu(display("failed to validate package manifest targets"))]
    Targets {
        source: ValidateManifestTargetsError,
    },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum VerifyManifestArtifactError {
    #[snafu(display("invalid package manifest"))]
    InvalidManifest { source: ValidateManifestError },
    #[snafu(display("failed to inspect package artifact"))]
    Metadata {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to open package artifact"))]
    Open {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to read package artifact"))]
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display(
        "package artifact size mismatch for {path:?}: expected {expected}, got {actual}"
    ))]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[snafu(display(
        "package artifact sha256 mismatch for {path:?}: expected {expected}, got {actual}"
    ))]
    Sha256Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}
