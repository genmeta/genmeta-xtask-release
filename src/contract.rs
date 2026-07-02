use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use semver::Version;
use serde::Deserialize;
use snafu::{ResultExt, Snafu};

use crate::{
    package::PackageId,
    system::{ArchitectureClass, PackageSystem},
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct ReleaseContract {
    pub package: BTreeMap<PackageId, PackageContract>,
    pub destination: DestinationContract,
}

impl ReleaseContract {
    pub fn validate(&self) -> Result<(), ValidateContractError> {
        for (id, package) in &self.package {
            package.validate(id)?;
        }
        for (id, package) in &self.package {
            for branch in package.branches() {
                validate_branch_requires(self, id, branch)?;
            }
        }
        validate_destination_env_refs(self)?;
        validate_destination_branches(self)?;
        Ok(())
    }

    pub fn package(&self, id: &str) -> Option<&PackageContract> {
        self.package
            .iter()
            .find(|(package_id, _)| package_id.as_str() == id)
            .map(|(_, package)| package)
    }

    pub fn package_entry(&self, id: &str) -> Option<(&PackageId, &PackageContract)> {
        self.package
            .iter()
            .find(|(package_id, _)| package_id.as_str() == id)
    }
}

pub fn load_release_contract(path: &Path) -> Result<ReleaseContract, LoadReleaseContractError> {
    let input = fs::read_to_string(path).context(load_release_contract_error::ReadSnafu {
        path: path.to_path_buf(),
    })?;
    let contract: ReleaseContract =
        toml::from_str(&input).context(load_release_contract_error::ParseSnafu {
            path: path.to_path_buf(),
        })?;
    contract
        .validate()
        .context(load_release_contract_error::InvalidSnafu)?;
    Ok(contract)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct PackageContract {
    pub manifest: Option<PathBuf>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    #[serde(default)]
    pub build: PackageBuildContract,
    pub deb: Option<DebBranch>,
    pub rpm: Option<RpmBranch>,
    pub brew: Option<BrewBranch>,
    pub scoop: Option<ScoopBranch>,
}

impl PackageContract {
    pub fn validate(&self, id: &PackageId) -> Result<(), ValidateContractError> {
        match (&self.manifest, &self.version) {
            (Some(_), Some(_)) => {
                return Err(ValidateContractError::ManifestAndVersion {
                    package: id.clone(),
                });
            }
            (None, None) => {
                return Err(ValidateContractError::MissingVersion {
                    package: id.clone(),
                });
            }
            (Some(_), None) => {}
            (None, Some(version)) => {
                validate_source_version(id, version)?;
                require_non_cargo_metadata(id, "description", &self.description)?;
                require_non_cargo_metadata(id, "license", &self.license)?;
                require_non_cargo_metadata(id, "homepage", &self.homepage)?;
            }
        }

        validate_env_bindings(id, None, &self.build.env)?;
        for branch in self.branches() {
            branch.validate(id)?;
        }
        Ok(())
    }

    pub fn branch(&self, system: PackageSystem) -> Option<PackageBranchRef<'_>> {
        match system {
            PackageSystem::Deb => self.deb.as_ref().map(PackageBranchRef::Deb),
            PackageSystem::Rpm => self.rpm.as_ref().map(PackageBranchRef::Rpm),
            PackageSystem::Brew => self.brew.as_ref().map(PackageBranchRef::Brew),
            PackageSystem::Scoop => self.scoop.as_ref().map(PackageBranchRef::Scoop),
        }
    }

    pub fn branches(&self) -> impl Iterator<Item = PackageBranchRef<'_>> {
        [
            self.branch(PackageSystem::Deb),
            self.branch(PackageSystem::Rpm),
            self.branch(PackageSystem::Brew),
            self.branch(PackageSystem::Scoop),
        ]
        .into_iter()
        .flatten()
    }
}

fn validate_source_version(id: &PackageId, version: &str) -> Result<(), ValidateContractError> {
    Version::parse(version).context(validate_contract_error::InvalidSourceVersionSnafu {
        package: id.clone(),
        version: version.to_owned(),
    })?;
    Ok(())
}

fn validate_env_bindings(
    id: &PackageId,
    system: Option<PackageSystem>,
    bindings: &BTreeMap<String, EnvBinding>,
) -> Result<(), ValidateContractError> {
    for (name, binding) in bindings {
        match (&binding.env, &binding.value) {
            (Some(env), None) => {
                if env.is_empty() {
                    return Err(ValidateContractError::EmptyEnvBinding {
                        package: id.clone(),
                        system,
                        name: name.clone(),
                    });
                }
                if binding
                    .container_path
                    .as_ref()
                    .is_some_and(|path| path.as_os_str().is_empty())
                {
                    return Err(ValidateContractError::EmptyContainerPath {
                        package: id.clone(),
                        system,
                        name: name.clone(),
                    });
                }
            }
            (None, Some(value)) => {
                if binding.container_path.is_some() {
                    return Err(ValidateContractError::ContainerPathValueBinding {
                        package: id.clone(),
                        system,
                        name: name.clone(),
                    });
                }
                if binding.optional {
                    return Err(ValidateContractError::OptionalValueBinding {
                        package: id.clone(),
                        system,
                        name: name.clone(),
                    });
                }
                if value.is_empty() {
                    return Err(ValidateContractError::EmptyEnvBinding {
                        package: id.clone(),
                        system,
                        name: name.clone(),
                    });
                }
            }
            _ => {
                return Err(ValidateContractError::InvalidEnvBinding {
                    package: id.clone(),
                    system,
                    name: name.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_destination_env_refs(contract: &ReleaseContract) -> Result<(), ValidateContractError> {
    validate_env_ref("endpoint", &contract.destination.s3.endpoint)?;
    validate_env_ref("access key id", &contract.destination.s3.access_key_id)?;
    validate_env_ref(
        "secret access key",
        &contract.destination.s3.secret_access_key,
    )?;
    if let Some(branch) = &contract.destination.s3.brew {
        validate_env_ref("stable brew tap token", &branch.stable.tap.token)?;
        validate_env_ref("preview brew tap token", &branch.preview.tap.token)?;
    }
    if let Some(branch) = &contract.destination.s3.scoop {
        validate_env_ref("stable scoop bucket token", &branch.stable.bucket.token)?;
        validate_env_ref("preview scoop bucket token", &branch.preview.bucket.token)?;
    }
    if let Some(branch) = &contract.destination.s3.deb {
        validate_env_ref("deb signing key", &branch.signing.key)?;
        validate_env_ref("deb signing passphrase", &branch.signing.passphrase)?;
        validate_env_ref("deb signing fingerprint", &branch.signing.fingerprint)?;
    }
    Ok(())
}

fn validate_env_ref(field: &'static str, ref_: &EnvRef) -> Result<(), ValidateContractError> {
    if ref_.env.is_empty() {
        return Err(ValidateContractError::EmptyEnvRef { field });
    }
    Ok(())
}

fn require_non_cargo_metadata(
    id: &PackageId,
    field: &'static str,
    value: &Option<String>,
) -> Result<(), ValidateContractError> {
    if value.as_deref().is_none_or(str::is_empty) {
        return Err(ValidateContractError::MissingPackageMetadata {
            package: id.clone(),
            field,
        });
    }
    Ok(())
}

fn validate_branch_requires(
    contract: &ReleaseContract,
    id: &PackageId,
    branch: PackageBranchRef<'_>,
) -> Result<(), ValidateContractError> {
    let system = branch.system();
    for (dependency, _) in branch.requires().iter() {
        let Some(dependency_package) = contract.package.get(dependency) else {
            return Err(ValidateContractError::MissingRequiredPackage {
                package: id.clone(),
                system,
                dependency: dependency.clone(),
            });
        };
        if dependency_package.branch(system).is_none() {
            return Err(ValidateContractError::MissingRequiredPackageBranch {
                package: id.clone(),
                system,
                dependency: dependency.clone(),
            });
        }
    }
    Ok(())
}

fn validate_destination_branches(contract: &ReleaseContract) -> Result<(), ValidateContractError> {
    validate_destination_branch(
        contract,
        PackageSystem::Deb,
        contract.destination.s3.deb.is_some(),
    )?;
    validate_destination_branch(
        contract,
        PackageSystem::Rpm,
        contract.destination.s3.rpm.is_some(),
    )?;
    validate_destination_branch(
        contract,
        PackageSystem::Brew,
        contract.destination.s3.brew.is_some(),
    )?;
    validate_destination_branch(
        contract,
        PackageSystem::Scoop,
        contract.destination.s3.scoop.is_some(),
    )?;
    validate_destination_publish(contract)?;
    Ok(())
}

fn validate_destination_branch(
    contract: &ReleaseContract,
    system: PackageSystem,
    defined: bool,
) -> Result<(), ValidateContractError> {
    if !defined {
        return Ok(());
    }
    if contract
        .package
        .values()
        .any(|package| package.branch(system).is_some())
    {
        return Ok(());
    }
    Err(ValidateContractError::DestinationWithoutPackageBranch { system })
}

fn validate_destination_publish(_contract: &ReleaseContract) -> Result<(), ValidateContractError> {
    Ok(())
}

fn validate_non_empty_path(
    package: &PackageId,
    system: PackageSystem,
    field: &'static str,
    path: &Path,
) -> Result<(), ValidateContractError> {
    if path.as_os_str().is_empty() {
        return Err(ValidateContractError::EmptyBranchPath {
            package: package.clone(),
            system,
            field,
        });
    }
    Ok(())
}

fn validate_linux_executor(
    package: &PackageId,
    system: PackageSystem,
    dockerfile: Option<&PathBuf>,
) -> Result<(), ValidateContractError> {
    let dockerfile =
        dockerfile.ok_or_else(|| ValidateContractError::MissingDockerfileExecutor {
            package: package.clone(),
            system,
        })?;
    validate_non_empty_path(package, system, "dockerfile", dockerfile)
}

fn validate_local_script_executor(
    package: &PackageId,
    system: PackageSystem,
    script: Option<&PathBuf>,
    manifest_template: Option<&PathBuf>,
) -> Result<(), ValidateContractError> {
    let script = script.ok_or_else(|| ValidateContractError::MissingScriptExecutor {
        package: package.clone(),
        system,
    })?;
    validate_non_empty_path(package, system, "script", script)?;
    let manifest_template =
        manifest_template.ok_or_else(|| ValidateContractError::MissingManifestTemplate {
            package: package.clone(),
            system,
        })?;
    validate_non_empty_path(package, system, "manifest_template", manifest_template)
}

fn validate_linux_version_part(
    package: &PackageId,
    system: PackageSystem,
    field: &'static str,
    value: &str,
) -> Result<(), ValidateContractError> {
    if value.is_empty() {
        return Err(ValidateContractError::EmptyLinuxVersionPart {
            package: package.clone(),
            system,
            field,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct PackageBuildContract {
    #[serde(default)]
    pub env: BTreeMap<String, EnvBinding>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct EnvBinding {
    pub env: Option<String>,
    pub value: Option<String>,
    pub container_path: Option<PathBuf>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct DebBranch {
    pub revision: String,
    pub architecture: ArchitectureClass,
    pub dockerfile: Option<PathBuf>,
    #[serde(default)]
    pub requires: RequiresContract,
    #[serde(default)]
    pub target: BTreeMap<String, PackageTargetContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct RpmBranch {
    pub release: String,
    pub architecture: ArchitectureClass,
    pub dockerfile: Option<PathBuf>,
    #[serde(default)]
    pub requires: RequiresContract,
    #[serde(default)]
    pub target: BTreeMap<String, PackageTargetContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct BrewBranch {
    pub script: Option<PathBuf>,
    pub manifest_template: Option<PathBuf>,
    #[serde(default)]
    pub target: BTreeMap<String, PackageTargetContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct ScoopBranch {
    pub script: Option<PathBuf>,
    pub manifest_template: Option<PathBuf>,
    #[serde(default)]
    pub target: BTreeMap<String, PackageTargetContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct RequiresContract(pub BTreeMap<PackageId, RequiredPackageContract>);

impl RequiresContract {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, package: &str) -> Option<&RequiredPackageContract> {
        self.0
            .iter()
            .find(|(id, _)| id.as_str() == package)
            .map(|(_, contract)| contract)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PackageId, &RequiredPackageContract)> {
        self.0.iter()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct RequiredPackageContract {
    pub version: VersionBoundContract,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct VersionBoundContract {
    #[serde(rename = ">=")]
    pub minimum: Option<VersionBoundSourceContract>,
    #[serde(rename = "<=")]
    pub maximum: Option<VersionBoundSourceContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct VersionBoundSourceContract {
    pub from: VersionBoundSource,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionBoundSource {
    #[serde(rename = "self")]
    SelfPackage,
    #[serde(rename = "dependency")]
    DependencyPackage,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct PackageTargetContract {
    #[serde(default)]
    pub env: BTreeMap<String, EnvBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageBranchRef<'a> {
    Deb(&'a DebBranch),
    Rpm(&'a RpmBranch),
    Brew(&'a BrewBranch),
    Scoop(&'a ScoopBranch),
}

impl<'a> PackageBranchRef<'a> {
    pub fn system(self) -> PackageSystem {
        match self {
            Self::Deb(_) => PackageSystem::Deb,
            Self::Rpm(_) => PackageSystem::Rpm,
            Self::Brew(_) => PackageSystem::Brew,
            Self::Scoop(_) => PackageSystem::Scoop,
        }
    }

    pub fn requires(self) -> &'a RequiresContract {
        match self {
            Self::Deb(branch) => &branch.requires,
            Self::Rpm(branch) => &branch.requires,
            Self::Brew(_) | Self::Scoop(_) => {
                static EMPTY: std::sync::LazyLock<RequiresContract> =
                    std::sync::LazyLock::new(RequiresContract::default);
                &EMPTY
            }
        }
    }

    pub fn script(self) -> Option<&'a Path> {
        match self {
            Self::Deb(_) | Self::Rpm(_) => None,
            Self::Brew(branch) => branch.script.as_deref(),
            Self::Scoop(branch) => branch.script.as_deref(),
        }
    }

    pub fn dockerfile(self) -> Option<&'a Path> {
        match self {
            Self::Deb(branch) => branch.dockerfile.as_deref(),
            Self::Rpm(branch) => branch.dockerfile.as_deref(),
            Self::Brew(_) | Self::Scoop(_) => None,
        }
    }

    pub fn manifest_template(self) -> Option<&'a Path> {
        match self {
            Self::Deb(_) | Self::Rpm(_) => None,
            Self::Brew(branch) => branch.manifest_template.as_deref(),
            Self::Scoop(branch) => branch.manifest_template.as_deref(),
        }
    }

    pub fn target_env(self, target: &str) -> Option<&'a BTreeMap<String, EnvBinding>> {
        match self {
            Self::Deb(branch) => branch.target.get(target).map(|contract| &contract.env),
            Self::Rpm(branch) => branch.target.get(target).map(|contract| &contract.env),
            Self::Brew(branch) => branch.target.get(target).map(|contract| &contract.env),
            Self::Scoop(branch) => branch.target.get(target).map(|contract| &contract.env),
        }
    }

    pub fn architecture(self) -> Option<ArchitectureClass> {
        match self {
            Self::Deb(branch) => Some(branch.architecture),
            Self::Rpm(branch) => Some(branch.architecture),
            Self::Brew(_) | Self::Scoop(_) => None,
        }
    }

    pub fn validate(self, id: &PackageId) -> Result<(), ValidateContractError> {
        match self {
            Self::Deb(branch) => {
                validate_linux_executor(id, PackageSystem::Deb, branch.dockerfile.as_ref())?;
                validate_linux_version_part(id, PackageSystem::Deb, "revision", &branch.revision)?;
                for target in branch.target.values() {
                    validate_env_bindings(id, Some(PackageSystem::Deb), &target.env)?;
                }
            }
            Self::Rpm(branch) => {
                validate_linux_executor(id, PackageSystem::Rpm, branch.dockerfile.as_ref())?;
                validate_linux_version_part(id, PackageSystem::Rpm, "release", &branch.release)?;
                for target in branch.target.values() {
                    validate_env_bindings(id, Some(PackageSystem::Rpm), &target.env)?;
                }
            }
            Self::Brew(branch) => {
                validate_local_script_executor(
                    id,
                    PackageSystem::Brew,
                    branch.script.as_ref(),
                    branch.manifest_template.as_ref(),
                )?;
                for target in branch.target.values() {
                    validate_env_bindings(id, Some(PackageSystem::Brew), &target.env)?;
                }
            }
            Self::Scoop(branch) => {
                validate_local_script_executor(
                    id,
                    PackageSystem::Scoop,
                    branch.script.as_ref(),
                    branch.manifest_template.as_ref(),
                )?;
                for target in branch.target.values() {
                    validate_env_bindings(id, Some(PackageSystem::Scoop), &target.env)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct DestinationContract {
    pub s3: S3DestinationContract,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3DestinationContract {
    pub bucket: String,
    pub endpoint: EnvRef,
    pub access_key_id: EnvRef,
    pub secret_access_key: EnvRef,
    pub brew: Option<S3BrewDestination>,
    pub scoop: Option<S3ScoopDestination>,
    pub deb: Option<S3DebDestination>,
    pub rpm: Option<S3RpmDestination>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct EnvRef {
    pub env: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3BrewDestination {
    pub stable: S3BrewChannelDestination,
    pub preview: S3BrewChannelDestination,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3BrewChannelDestination {
    pub prefix: String,
    pub public_base_url: String,
    pub tap: GitIndexDestination,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3ScoopDestination {
    pub stable: S3ScoopChannelDestination,
    pub preview: S3ScoopChannelDestination,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3ScoopChannelDestination {
    pub prefix: String,
    pub public_base_url: String,
    pub bucket: GitIndexDestination,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct GitIndexDestination {
    pub repository: String,
    pub base_branch: String,
    pub token: EnvRef,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3DebDestination {
    pub stable: S3DebChannelDestination,
    pub preview: S3DebChannelDestination,
    pub signing: DebSigning,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3DebChannelDestination {
    pub prefix: String,
    pub suite: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct DebSigning {
    pub key: EnvRef,
    pub passphrase: EnvRef,
    pub fingerprint: EnvRef,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3RpmDestination {
    pub stable: S3RpmChannelDestination,
    pub preview: S3RpmChannelDestination,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub struct S3RpmChannelDestination {
    pub prefix: String,
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ValidateContractError {
    #[snafu(display("package {package} must not define both manifest and version"))]
    ManifestAndVersion { package: PackageId },
    #[snafu(display("package {package} must define manifest or version"))]
    MissingVersion { package: PackageId },
    #[snafu(display("package {package} root version {version} must be a source version"))]
    PackageSystemVersionAtRoot { package: PackageId, version: String },
    #[snafu(display("package {package} has invalid source version {version}"))]
    InvalidSourceVersion {
        source: semver::Error,
        package: PackageId,
        version: String,
    },
    #[snafu(display("package {package} missing metadata field {field}"))]
    MissingPackageMetadata {
        package: PackageId,
        field: &'static str,
    },
    #[snafu(display("package {package} {system} branch must use dockerfile executor"))]
    MissingDockerfileExecutor {
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("package {package} {system} branch must use script executor"))]
    MissingScriptExecutor {
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("package {package} {system} branch missing manifest_template"))]
    MissingManifestTemplate {
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("package {package} {system} branch {field} must not be empty"))]
    EmptyBranchPath {
        package: PackageId,
        system: PackageSystem,
        field: &'static str,
    },
    #[snafu(display("package {package} {system} branch {field} must not be empty"))]
    EmptyLinuxVersionPart {
        package: PackageId,
        system: PackageSystem,
        field: &'static str,
    },
    #[snafu(display("package {package} {system} branch requires missing package {dependency}"))]
    MissingRequiredPackage {
        package: PackageId,
        system: PackageSystem,
        dependency: PackageId,
    },
    #[snafu(display(
        "package {package} {system} branch requires package {dependency} without {system} branch"
    ))]
    MissingRequiredPackageBranch {
        package: PackageId,
        system: PackageSystem,
        dependency: PackageId,
    },
    #[snafu(display("destination s3 {system} branch has no package {system} branch"))]
    DestinationWithoutPackageBranch { system: PackageSystem },
    #[snafu(display("package {package} env binding {name} must set exactly one of env or value"))]
    InvalidEnvBinding {
        package: PackageId,
        system: Option<PackageSystem>,
        name: String,
    },
    #[snafu(display("package {package} env binding {name} must not be empty"))]
    EmptyEnvBinding {
        package: PackageId,
        system: Option<PackageSystem>,
        name: String,
    },
    #[snafu(display("package {package} env binding {name} optional requires env"))]
    OptionalValueBinding {
        package: PackageId,
        system: Option<PackageSystem>,
        name: String,
    },
    #[snafu(display("package {package} env binding {name} container path requires env"))]
    ContainerPathValueBinding {
        package: PackageId,
        system: Option<PackageSystem>,
        name: String,
    },
    #[snafu(display("package {package} env binding {name} container path must not be empty"))]
    EmptyContainerPath {
        package: PackageId,
        system: Option<PackageSystem>,
        name: String,
    },
    #[snafu(display("destination s3 {field} env ref must not be empty"))]
    EmptyEnvRef { field: &'static str },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LoadReleaseContractError {
    #[snafu(display("failed to read release contract"))]
    Read {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to parse release contract"))]
    Parse {
        source: toml::de::Error,
        path: PathBuf,
    },
    #[snafu(display("invalid release contract"))]
    Invalid { source: ValidateContractError },
}

#[cfg(test)]
mod tests {
    use super::ReleaseContract;

    fn parse_contract(input: &str) -> Result<ReleaseContract, toml::de::Error> {
        toml::from_str(input)
    }

    #[test]
    fn parses_channel_aware_s3_destinations() {
        let contract = parse_contract(
            r#"
                [package.gmutils]
                version = "0.8.0-beta.1"
                description = "Genmeta CLI"
                license = "Apache-2.0"
                homepage = "https://www.dhttp.net"

                [package.gmutils.deb]
                revision = "1"
                architecture = "target"
                dockerfile = "xtask/release/deb/Dockerfile"

                [package.gmutils.rpm]
                release = "1"
                architecture = "target"
                dockerfile = "xtask/release/rpm/Dockerfile"

                [package.gmutils.brew]
                script = "xtask/release/brew/gmutils.sh"
                manifest_template = "xtask/templates/gmutils.rb.in"

                [package.gmutils.scoop]
                script = "xtask/release/scoop/gmutils.sh"
                manifest_template = "xtask/templates/gmutils.json.in"

                [destination.s3]
                bucket = "download"
                endpoint.env = "S3_ENDPOINT"
                access_key_id.env = "S3_ACCESS_KEY_ID"
                secret_access_key.env = "S3_SECRET_ACCESS_KEY"

                [destination.s3.deb.stable]
                prefix = "ppa/genmeta"
                suite = "stable"

                [destination.s3.deb.preview]
                prefix = "ppa/genmeta"
                suite = "preview"

                [destination.s3.deb.signing]
                key.env = "APT_SIGNING_KEY"
                passphrase.env = "APT_SIGNING_PASSPHRASE"
                fingerprint.env = "APT_SIGNING_FINGERPRINT"

                [destination.s3.rpm.stable]
                prefix = "rpm/stable"

                [destination.s3.rpm.preview]
                prefix = "rpm/preview"

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

                [destination.s3.scoop.stable]
                prefix = "scoop/stable"
                public_base_url = "https://download.dhttp.net/scoop/stable"
                bucket.repository = "genmeta/scoop-stable"
                bucket.base_branch = "main"
                bucket.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"

                [destination.s3.scoop.preview]
                prefix = "scoop/preview"
                public_base_url = "https://download.dhttp.net/scoop/preview"
                bucket.repository = "genmeta/scoop-preview"
                bucket.base_branch = "main"
                bucket.token.env = "HOMEBREW_TAP_GITHUB_TOKEN"
            "#,
        )
        .expect("new channel schema should parse");

        contract
            .validate()
            .expect("new channel schema should validate");
        assert_eq!(
            contract.destination.s3.deb.as_ref().unwrap().stable.suite,
            "stable"
        );
        assert_eq!(
            contract.destination.s3.deb.as_ref().unwrap().preview.suite,
            "preview"
        );
        assert_eq!(
            contract.destination.s3.rpm.as_ref().unwrap().stable.prefix,
            "rpm/stable"
        );
        assert_eq!(
            contract.destination.s3.rpm.as_ref().unwrap().preview.prefix,
            "rpm/preview"
        );
    }

    #[test]
    fn rejects_old_single_channel_deb_destination_shape() {
        let error = parse_contract(
            r#"
                [package.product]
                version = "1.2.3"
                description = "product"
                license = "Apache-2.0"
                homepage = "https://example.test"

                [package.product.deb]
                revision = "1"
                architecture = "target"
                dockerfile = "xtask/release/deb/Dockerfile"

                [destination.s3]
                bucket = "download"
                endpoint.env = "S3_ENDPOINT"
                access_key_id.env = "S3_ACCESS_KEY_ID"
                secret_access_key.env = "S3_SECRET_ACCESS_KEY"

                [destination.s3.deb]
                prefix = "ppa/genmeta"
                suite = "genmeta"
                signing.key.env = "APT_SIGNING_KEY"
                signing.passphrase.env = "APT_SIGNING_PASSPHRASE"
            "#,
        )
        .expect_err("old schema must be rejected by serde deny_unknown_fields");

        assert!(error.to_string().contains("unknown field"));
    }
}
