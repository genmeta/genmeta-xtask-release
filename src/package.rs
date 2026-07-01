use std::{fmt, str::FromStr};

use semver::Version;
use serde::{Deserialize, Serialize};
use snafu::Snafu;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageId(String);

impl PackageId {
    pub fn new(value: impl Into<String>) -> Result<Self, ParsePackageIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ParsePackageIdError::Empty);
        }
        if value
            .bytes()
            .any(|byte| byte == b'.' || byte.is_ascii_whitespace())
        {
            return Err(ParsePackageIdError::Invalid { value });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn env_fragment(&self) -> String {
        self.0
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect()
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PackageId {
    type Err = ParsePackageIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Debug, Snafu, PartialEq, Eq)]
#[snafu(module)]
pub enum ParsePackageIdError {
    #[snafu(display("package id must not be empty"))]
    Empty,
    #[snafu(display("invalid package id {value}"))]
    Invalid { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackageMetadata {
    pub source_version: Version,
    pub description: String,
    pub license: String,
    pub homepage: String,
    pub repository: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageVersion {
    Deb { source: Version, revision: String },
    Rpm { source: Version, release: String },
    Plain { source: Version },
}

impl PackageVersion {
    pub fn deb(source: Version, revision: impl Into<String>) -> Result<Self, PackageVersionError> {
        let revision = revision.into();
        ensure_segment(&revision, "deb revision")?;
        Ok(Self::Deb { source, revision })
    }

    pub fn rpm(source: Version, release: impl Into<String>) -> Result<Self, PackageVersionError> {
        let release = release.into();
        ensure_segment(&release, "rpm release")?;
        Ok(Self::Rpm { source, release })
    }

    pub fn plain(source: Version) -> Self {
        Self::Plain { source }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::Deb { source, revision } => {
                format!("{}-{revision}", linux_source_version(source))
            }
            Self::Rpm { source, release } => format!("{}-{release}", linux_source_version(source)),
            Self::Plain { source } => source.to_string(),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PackageVersionError {
    #[snafu(display("{name} must not be empty"))]
    EmptySegment { name: &'static str },
}

fn ensure_segment(value: &str, name: &'static str) -> Result<(), PackageVersionError> {
    if value.is_empty() {
        return Err(PackageVersionError::EmptySegment { name });
    }
    Ok(())
}

fn linux_source_version(source: &Version) -> String {
    let mut version = format!("{}.{}.{}", source.major, source.minor, source.patch);
    if !source.pre.is_empty() {
        version.push('~');
        version.push_str(source.pre.as_str());
    }
    if !source.build.is_empty() {
        version.push('+');
        version.push_str(source.build.as_str());
    }
    version
}

use std::path::{Path, PathBuf};

use cargo_metadata::MetadataCommand;
use snafu::{OptionExt, ResultExt};

use crate::contract::ReleaseContract;

pub fn resolve_metadata(
    contract: &ReleaseContract,
    package: &str,
    contract_root: &Path,
) -> Result<ResolvedPackageMetadata, ResolvePackageMetadataError> {
    let (_, package_contract) = contract.package_entry(package).context(
        resolve_package_metadata_error::MissingPackageSnafu {
            package: package.to_owned(),
        },
    )?;

    if let Some(manifest) = &package_contract.manifest {
        return resolve_cargo_metadata(package, contract_root, manifest);
    }

    let version = package_contract.version.as_deref().context(
        resolve_package_metadata_error::MissingVersionSnafu {
            package: package.to_owned(),
        },
    )?;
    let source_version = Version::parse(version).context(
        resolve_package_metadata_error::InvalidSourceVersionSnafu {
            package: package.to_owned(),
            version: version.to_owned(),
        },
    )?;
    Ok(ResolvedPackageMetadata {
        source_version,
        description: required_metadata_field(
            package,
            "description",
            &package_contract.description,
        )?,
        license: required_metadata_field(package, "license", &package_contract.license)?,
        homepage: required_metadata_field(package, "homepage", &package_contract.homepage)?,
        repository: package_contract.repository.clone(),
    })
}

fn resolve_cargo_metadata(
    _package: &str,
    contract_root: &Path,
    manifest: &Path,
) -> Result<ResolvedPackageMetadata, ResolvePackageMetadataError> {
    let manifest = if manifest.is_absolute() {
        manifest.to_path_buf()
    } else {
        contract_root.join(manifest)
    };
    let metadata = MetadataCommand::new()
        .manifest_path(&manifest)
        .no_deps()
        .exec()
        .context(resolve_package_metadata_error::CargoMetadataSnafu {
            manifest: manifest.clone(),
        })?;
    let package = metadata
        .root_package()
        .or_else(|| {
            metadata
                .packages
                .iter()
                .find(|candidate| candidate.manifest_path.as_std_path() == manifest)
        })
        .context(resolve_package_metadata_error::MissingCargoPackageSnafu { manifest })?;

    Ok(ResolvedPackageMetadata {
        source_version: package.version.clone(),
        description: package.description.clone().context(
            resolve_package_metadata_error::MissingCargoMetadataSnafu {
                package: package.name.to_string(),
                field: "description",
            },
        )?,
        license: package.license.clone().context(
            resolve_package_metadata_error::MissingCargoMetadataSnafu {
                package: package.name.to_string(),
                field: "license",
            },
        )?,
        homepage: package.homepage.clone().context(
            resolve_package_metadata_error::MissingCargoMetadataSnafu {
                package: package.name.to_string(),
                field: "homepage",
            },
        )?,
        repository: package.repository.clone(),
    })
}

fn required_metadata_field(
    package: &str,
    field: &'static str,
    value: &Option<String>,
) -> Result<String, ResolvePackageMetadataError> {
    value.clone().filter(|value| !value.is_empty()).context(
        resolve_package_metadata_error::MissingExplicitMetadataSnafu {
            package: package.to_owned(),
            field,
        },
    )
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ResolvePackageMetadataError {
    #[snafu(display("package {package} does not exist"))]
    MissingPackage { package: String },
    #[snafu(display("package {package} is missing source version"))]
    MissingVersion { package: String },
    #[snafu(display("package {package} has invalid source version {version}"))]
    InvalidSourceVersion {
        source: semver::Error,
        package: String,
        version: String,
    },
    #[snafu(display("package {package} is missing metadata field {field}"))]
    MissingExplicitMetadata {
        package: String,
        field: &'static str,
    },
    #[snafu(display("failed to read cargo metadata"))]
    CargoMetadata {
        source: cargo_metadata::Error,
        manifest: PathBuf,
    },
    #[snafu(display("cargo metadata did not include package for manifest"))]
    MissingCargoPackage { manifest: PathBuf },
    #[snafu(display("cargo package {package} is missing metadata field {field}"))]
    MissingCargoMetadata {
        package: String,
        field: &'static str,
    },
}
