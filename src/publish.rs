use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use semver::Version;
use snafu::{OptionExt, ResultExt, Snafu, ensure};

use crate::{
    channel::ReleaseChannel,
    contract::{EnvRef, ReleaseContract},
    manifest::PackageManifest,
    package::PackageId,
    system::PackageSystem,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePublishSurface {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomatedPublishDecision<T> {
    ManualInitialPublicationRequired,
    Publish { payloads: Vec<T> },
}

pub fn plan_automated_publish<T>(
    surface: RemotePublishSurface,
    payloads: Vec<T>,
) -> AutomatedPublishDecision<T> {
    match surface {
        RemotePublishSurface::Missing => AutomatedPublishDecision::ManualInitialPublicationRequired,
        RemotePublishSurface::Present => AutomatedPublishDecision::Publish { payloads },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemotePayloadState {
    Missing,
    Present { sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadCondition {
    IfMissing,
    IfMatch(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedImmutablePayloadPlan {
    metadata_sha256: String,
    upload_condition: Option<UploadCondition>,
    remote_sha256_matches_local: Option<bool>,
}

impl VersionedImmutablePayloadPlan {
    pub fn metadata_sha256(&self) -> &str {
        &self.metadata_sha256
    }

    pub fn upload_condition(&self) -> Option<UploadCondition> {
        self.upload_condition.clone()
    }

    pub fn reuses_remote_payload(&self) -> bool {
        self.remote_sha256_matches_local.is_some()
    }

    pub fn remote_sha256_matches_local(&self) -> bool {
        self.remote_sha256_matches_local.unwrap_or(false)
    }
}

pub fn plan_immutable_upload(
    payload_path: &str,
    actual_sha256: &str,
    remote: RemotePayloadState,
) -> Result<Option<UploadCondition>, ImmutableCollisionError> {
    match remote {
        RemotePayloadState::Missing => Ok(Some(UploadCondition::IfMissing)),
        RemotePayloadState::Present { sha256 } if sha256 == actual_sha256 => Ok(None),
        RemotePayloadState::Present { sha256 } => Err(ImmutableCollisionError::DifferentHash {
            payload_path: payload_path.to_string(),
            sha256,
        }),
    }
}

pub fn plan_versioned_immutable_payload(
    _payload_path: &str,
    actual_sha256: &str,
    remote: RemotePayloadState,
) -> VersionedImmutablePayloadPlan {
    match remote {
        RemotePayloadState::Missing => VersionedImmutablePayloadPlan {
            metadata_sha256: actual_sha256.to_string(),
            upload_condition: Some(UploadCondition::IfMissing),
            remote_sha256_matches_local: None,
        },
        RemotePayloadState::Present { sha256 } => {
            let matches_local = sha256 == actual_sha256;
            VersionedImmutablePayloadPlan {
                metadata_sha256: sha256,
                upload_condition: None,
                remote_sha256_matches_local: Some(matches_local),
            }
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ImmutableCollisionError {
    #[snafu(display(
        "remote immutable payload {payload_path} already exists with different sha256 {sha256}"
    ))]
    DifferentHash {
        payload_path: String,
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxPackageVersion {
    pub package: String,
    pub version: String,
    pub architecture: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDebPackageEntry {
    pub version: LinuxPackageVersion,
    pub filename: String,
}

pub fn remote_deb_package_entries_from_packages(
    content: &str,
) -> Result<Vec<RemoteDebPackageEntry>, RemoteDebPackageEntriesFromPackagesError> {
    let mut entries = Vec::new();
    for stanza in content.split("\n\n") {
        let stanza = stanza.trim();
        if stanza.is_empty() {
            continue;
        }
        let package = deb_stanza_field(stanza, "Package").context(
            remote_deb_package_entries_from_packages_error::MissingFieldSnafu { field: "Package" },
        )?;
        let version = deb_stanza_field(stanza, "Version").context(
            remote_deb_package_entries_from_packages_error::MissingFieldSnafu { field: "Version" },
        )?;
        let architecture = deb_stanza_field(stanza, "Architecture").context(
            remote_deb_package_entries_from_packages_error::MissingFieldSnafu {
                field: "Architecture",
            },
        )?;
        let filename = deb_stanza_field(stanza, "Filename").context(
            remote_deb_package_entries_from_packages_error::MissingFieldSnafu { field: "Filename" },
        )?;
        entries.push(RemoteDebPackageEntry {
            version: LinuxPackageVersion {
                package,
                version,
                architecture,
            },
            filename,
        });
    }
    Ok(entries)
}

pub fn remote_deb_package_entries_from_keys<'a>(
    prefix: &RemotePrefix,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<RemoteDebPackageEntry>, RemoteDebPackageEntriesFromKeysError> {
    let mut entries = Vec::new();
    for key in keys {
        let version = remote_deb_payload_version_from_key(prefix, key)
            .context(remote_deb_package_entries_from_keys_error::VersionSnafu)?;
        let filename = key
            .strip_prefix(prefix.as_str())
            .and_then(|value| value.strip_prefix('/'))
            .context(remote_deb_package_entries_from_keys_error::UnexpectedLayoutSnafu { key })?;
        entries.push(RemoteDebPackageEntry {
            version,
            filename: filename.to_string(),
        });
    }
    Ok(entries)
}

fn deb_stanza_field(stanza: &str, name: &'static str) -> Option<String> {
    let prefix = format!("{name}:");
    stanza.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(|value| value.trim().to_string())
    })
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemoteDebPackageEntriesFromPackagesError {
    #[snafu(display("remote deb package stanza is missing {field}"))]
    MissingField { field: &'static str },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemoteDebPackageEntriesFromKeysError {
    #[snafu(display("failed to parse remote deb payload version"))]
    Version {
        source: RemoteDebPayloadVersionFromKeyError,
    },
    #[snafu(display("remote deb payload key {key} has unexpected layout"))]
    UnexpectedLayout { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxPackagePayload {
    pub package: String,
    pub version: String,
    pub architecture: String,
    pub archive_name: String,
    pub path: String,
}

pub fn linux_package_payloads_from_manifest(
    manifest: &PackageManifest,
) -> Result<Vec<LinuxPackagePayload>, LinuxPackagePayloadsFromManifestError> {
    ensure!(
        matches!(manifest.kind, PackageSystem::Deb | PackageSystem::Rpm),
        linux_package_payloads_from_manifest_error::WrongKindSnafu {
            system: manifest.kind
        }
    );
    manifest
        .artifacts
        .iter()
        .map(|artifact| {
            Ok(LinuxPackagePayload {
                package: artifact
                    .package_name
                    .clone()
                    .context(linux_package_payloads_from_manifest_error::MissingPackageNameSnafu)?,
                version: artifact.package_version.clone().context(
                    linux_package_payloads_from_manifest_error::MissingPackageVersionSnafu,
                )?,
                architecture: artifact.architecture.clone().context(
                    linux_package_payloads_from_manifest_error::MissingArchitectureSnafu,
                )?,
                archive_name: artifact
                    .archive_name
                    .clone()
                    .context(linux_package_payloads_from_manifest_error::MissingArchiveNameSnafu)?,
                path: artifact.path.clone(),
            })
        })
        .collect()
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LinuxPackagePayloadsFromManifestError {
    #[snafu(display("{system} package manifest does not contain linux package payloads"))]
    WrongKind { system: PackageSystem },
    #[snafu(display("linux package artifact is missing package name"))]
    MissingPackageName,
    #[snafu(display("linux package artifact is missing package version"))]
    MissingPackageVersion,
    #[snafu(display("linux package artifact is missing architecture"))]
    MissingArchitecture,
    #[snafu(display("linux package artifact is missing archive name"))]
    MissingArchiveName,
}

pub fn linux_payload_key(
    prefix: &RemotePrefix,
    system: PackageSystem,
    payload: &LinuxPackagePayload,
) -> Result<String, LinuxPayloadKeyError> {
    match system {
        PackageSystem::Deb => {
            let first =
                payload
                    .package
                    .chars()
                    .next()
                    .ok_or(LinuxPayloadKeyError::EmptyPackageName {
                        system: PackageSystem::Deb,
                    })?;
            Ok(prefix.join(&format!(
                "pool/main/{first}/{}/{}",
                payload.package, payload.archive_name
            )))
        }
        PackageSystem::Rpm => Ok(prefix.join(&format!(
            "{}/{}/{}",
            payload.package, payload.version, payload.archive_name
        ))),
        PackageSystem::Brew | PackageSystem::Scoop => {
            Err(LinuxPayloadKeyError::UnsupportedSystem { system })
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LinuxPayloadKeyError {
    #[snafu(display("{system} package payload keys are not linux package keys"))]
    UnsupportedSystem { system: PackageSystem },
    #[snafu(display("{system} package payload has empty package name"))]
    EmptyPackageName { system: PackageSystem },
}

pub fn remote_deb_payload_version_from_key(
    prefix: &RemotePrefix,
    key: &str,
) -> Result<LinuxPackageVersion, RemoteDebPayloadVersionFromKeyError> {
    let relative = key
        .strip_prefix(prefix.as_str())
        .and_then(|value| value.strip_prefix('/'))
        .context(remote_deb_payload_version_from_key_error::UnexpectedLayoutSnafu { key })?;
    let parts = relative.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() == 5 && parts[0] == "pool" && parts[1] == "main",
        remote_deb_payload_version_from_key_error::UnexpectedLayoutSnafu { key }
    );
    let archive_name = parts[4];
    let stem = archive_name
        .strip_suffix(".deb")
        .context(remote_deb_payload_version_from_key_error::MissingDebFilenameSnafu { key })?;
    let (package_and_version, architecture) = stem
        .rsplit_once('_')
        .context(remote_deb_payload_version_from_key_error::MissingDebFilenameSnafu { key })?;
    let (package, version) = package_and_version
        .split_once('_')
        .context(remote_deb_payload_version_from_key_error::MissingDebFilenameSnafu { key })?;
    Ok(LinuxPackageVersion {
        package: package.to_string(),
        version: version.to_string(),
        architecture: architecture.to_string(),
    })
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemoteDebPayloadVersionFromKeyError {
    #[snafu(display("remote deb payload key {key} has unexpected layout"))]
    UnexpectedLayout { key: String },
    #[snafu(display("remote deb payload key {key} is missing deb filename"))]
    MissingDebFilename { key: String },
}

pub fn remote_rpm_payload_version_from_key(
    prefix: &RemotePrefix,
    key: &str,
) -> Result<LinuxPackageVersion, RemoteRpmPayloadVersionFromKeyError> {
    let relative = key
        .strip_prefix(prefix.as_str())
        .and_then(|value| value.strip_prefix('/'))
        .context(remote_rpm_payload_version_from_key_error::UnexpectedLayoutSnafu { key })?;
    let parts = relative.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() >= 3,
        remote_rpm_payload_version_from_key_error::UnexpectedLayoutSnafu { key }
    );
    let archive_name = parts[parts.len() - 1];
    let architecture = archive_name
        .strip_suffix(".rpm")
        .and_then(|stem| stem.rsplit_once('.').map(|(_, architecture)| architecture))
        .context(remote_rpm_payload_version_from_key_error::MissingRpmFilenameSnafu { key })?;
    Ok(LinuxPackageVersion {
        package: parts[0].to_string(),
        version: parts[1].to_string(),
        architecture: architecture.to_string(),
    })
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemoteRpmPayloadVersionFromKeyError {
    #[snafu(display("remote rpm payload key {key} has unexpected layout"))]
    UnexpectedLayout { key: String },
    #[snafu(display("remote rpm payload key {key} is missing rpm filename"))]
    MissingRpmFilename { key: String },
}

pub fn remote_linux_payload_versions_from_keys<'a>(
    system: PackageSystem,
    prefix: &RemotePrefix,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<LinuxPackageVersion>, RemoteLinuxPayloadVersionsFromKeysError> {
    match system {
        PackageSystem::Deb => keys
            .into_iter()
            .map(|key| {
                remote_deb_payload_version_from_key(prefix, key)
                    .context(remote_linux_payload_versions_from_keys_error::DebSnafu)
            })
            .collect(),
        PackageSystem::Rpm => keys
            .into_iter()
            .map(|key| {
                remote_rpm_payload_version_from_key(prefix, key)
                    .context(remote_linux_payload_versions_from_keys_error::RpmSnafu)
            })
            .collect(),
        PackageSystem::Brew | PackageSystem::Scoop => {
            Err(RemoteLinuxPayloadVersionsFromKeysError::UnsupportedSystem { system })
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemoteLinuxPayloadVersionsFromKeysError {
    #[snafu(display("{system} package systems do not define linux payload versions"))]
    UnsupportedSystem { system: PackageSystem },
    #[snafu(display("failed to parse remote deb payload version"))]
    Deb {
        source: RemoteDebPayloadVersionFromKeyError,
    },
    #[snafu(display("failed to parse remote rpm payload version"))]
    Rpm {
        source: RemoteRpmPayloadVersionFromKeyError,
    },
}

pub fn linux_repository_upload_order(
    system: PackageSystem,
    key: &str,
) -> Result<u8, LinuxRepositoryUploadOrderError> {
    match system {
        PackageSystem::Deb => Ok(deb_repository_upload_order(key)),
        PackageSystem::Rpm => Ok(rpm_repository_upload_order(key)),
        PackageSystem::Brew | PackageSystem::Scoop => {
            Err(LinuxRepositoryUploadOrderError::UnsupportedSystem { system })
        }
    }
}

fn deb_repository_upload_order(key: &str) -> u8 {
    if key.contains("/pool/") || key.starts_with("pool/") {
        return 0;
    }
    if key.contains("/binary-") {
        return 1;
    }
    if key.ends_with("InRelease") {
        return 4;
    }
    if key.ends_with("Release.gpg") {
        return 3;
    }
    2
}

fn rpm_repository_upload_order(key: &str) -> u8 {
    if key.ends_with(".rpm") {
        return 0;
    }
    if key.ends_with("repodata/repomd.xml") {
        return 4;
    }
    2
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum LinuxRepositoryUploadOrderError {
    #[snafu(display("{system} repositories do not define linux metadata upload order"))]
    UnsupportedSystem { system: PackageSystem },
}

pub fn select_publishable_linux_payloads<E>(
    payloads: Vec<LinuxPackagePayload>,
    remote_versions: &[LinuxPackageVersion],
    compare_versions: impl Fn(&str, &str) -> Result<Ordering, E>,
) -> Result<Vec<LinuxPackagePayload>, E> {
    let mut latest_remote = BTreeMap::<(&str, &str), &str>::new();
    for version in remote_versions {
        let key = (version.package.as_str(), version.architecture.as_str());
        match latest_remote.get(&key) {
            None => {
                latest_remote.insert(key, version.version.as_str());
            }
            Some(current) => {
                if compare_versions(&version.version, current)?.is_gt() {
                    latest_remote.insert(key, version.version.as_str());
                }
            }
        }
    }

    let mut selected = Vec::new();
    for payload in payloads {
        let key = (payload.package.as_str(), payload.architecture.as_str());
        let should_publish = match latest_remote.get(&key) {
            None => true,
            Some(remote_version) => compare_versions(&payload.version, remote_version)?.is_gt(),
        };
        if should_publish {
            selected.push(payload);
        }
    }
    Ok(selected)
}

pub fn publishable_linux_payloads_from_manifest_and_remote_keys<'a, E>(
    manifest: &PackageManifest,
    prefix: &RemotePrefix,
    remote_keys: impl IntoIterator<Item = &'a str>,
    compare_versions: impl Fn(&str, &str) -> Result<Ordering, E>,
) -> Result<Vec<LinuxPackagePayload>, PublishableLinuxPayloadsFromManifestAndRemoteKeysError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let local_payloads = linux_package_payloads_from_manifest(manifest).context(
        publishable_linux_payloads_from_manifest_and_remote_keys_error::LocalPayloadsSnafu,
    )?;
    let remote_versions =
        remote_linux_payload_versions_from_keys(manifest.kind, prefix, remote_keys).context(
            publishable_linux_payloads_from_manifest_and_remote_keys_error::RemoteVersionsSnafu,
        )?;
    select_publishable_linux_payloads(local_payloads, &remote_versions, compare_versions).context(
        publishable_linux_payloads_from_manifest_and_remote_keys_error::CompareVersionsSnafu,
    )
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PublishableLinuxPayloadsFromManifestAndRemoteKeysError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[snafu(display("failed to read linux package payloads from manifest"))]
    LocalPayloads {
        source: LinuxPackagePayloadsFromManifestError,
    },
    #[snafu(display("failed to read remote linux payload versions"))]
    RemoteVersions {
        source: RemoteLinuxPayloadVersionsFromKeysError,
    },
    #[snafu(display("failed to select publishable linux payloads"))]
    CompareVersions { source: E },
}

pub fn publishable_deb_payloads_from_manifest_and_packages<'a, E>(
    manifest: &PackageManifest,
    packages_contents: impl IntoIterator<Item = &'a str>,
    compare_versions: impl Fn(&str, &str) -> Result<Ordering, E>,
) -> Result<Vec<LinuxPackagePayload>, PublishableDebPayloadsFromManifestAndPackagesError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    ensure!(
        manifest.kind == PackageSystem::Deb,
        publishable_deb_payloads_from_manifest_and_packages_error::WrongKindSnafu {
            system: manifest.kind
        }
    );
    let local_payloads = linux_package_payloads_from_manifest(manifest)
        .context(publishable_deb_payloads_from_manifest_and_packages_error::LocalPayloadsSnafu)?;
    let mut remote_versions = Vec::new();
    for content in packages_contents {
        let entries = remote_deb_package_entries_from_packages(content).context(
            publishable_deb_payloads_from_manifest_and_packages_error::RemotePackagesSnafu,
        )?;
        remote_versions.extend(entries.into_iter().map(|entry| entry.version));
    }
    select_publishable_linux_payloads(local_payloads, &remote_versions, compare_versions)
        .context(publishable_deb_payloads_from_manifest_and_packages_error::CompareVersionsSnafu)
}

pub fn publishable_deb_payloads_from_manifest_packages_and_remote_keys<'a, 'b, E>(
    manifest: &PackageManifest,
    packages_contents: impl IntoIterator<Item = &'a str>,
    prefix: &RemotePrefix,
    remote_keys: impl IntoIterator<Item = &'b str>,
    compare_versions: impl Fn(&str, &str) -> Result<Ordering, E>,
) -> Result<Vec<LinuxPackagePayload>, PublishableDebPayloadsFromManifestPackagesAndRemoteKeysError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    ensure!(
        manifest.kind == PackageSystem::Deb,
        publishable_deb_payloads_from_manifest_packages_and_remote_keys_error::WrongKindSnafu {
            system: manifest.kind
        }
    );
    let local_payloads = linux_package_payloads_from_manifest(manifest).context(
        publishable_deb_payloads_from_manifest_packages_and_remote_keys_error::LocalPayloadsSnafu,
    )?;
    let mut remote_versions = Vec::new();
    for content in packages_contents {
        let entries = remote_deb_package_entries_from_packages(content).context(
            publishable_deb_payloads_from_manifest_packages_and_remote_keys_error::RemotePackagesSnafu,
        )?;
        remote_versions.extend(entries.into_iter().map(|entry| entry.version));
    }
    let entries = remote_deb_package_entries_from_keys(prefix, remote_keys).context(
        publishable_deb_payloads_from_manifest_packages_and_remote_keys_error::RemoteKeysSnafu,
    )?;
    remote_versions.extend(entries.into_iter().map(|entry| entry.version));
    select_publishable_linux_payloads(local_payloads, &remote_versions, compare_versions).context(
        publishable_deb_payloads_from_manifest_packages_and_remote_keys_error::CompareVersionsSnafu,
    )
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PublishableDebPayloadsFromManifestAndPackagesError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[snafu(display("{system} package manifest is not a deb package manifest"))]
    WrongKind { system: PackageSystem },
    #[snafu(display("failed to read deb package payloads from manifest"))]
    LocalPayloads {
        source: LinuxPackagePayloadsFromManifestError,
    },
    #[snafu(display("failed to read remote deb package entries"))]
    RemotePackages {
        source: RemoteDebPackageEntriesFromPackagesError,
    },
    #[snafu(display("failed to select publishable deb payloads"))]
    CompareVersions { source: E },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PublishableDebPayloadsFromManifestPackagesAndRemoteKeysError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[snafu(display("{system} package manifest is not a deb package manifest"))]
    WrongKind { system: PackageSystem },
    #[snafu(display("failed to read deb package payloads from manifest"))]
    LocalPayloads {
        source: LinuxPackagePayloadsFromManifestError,
    },
    #[snafu(display("failed to read remote deb package entries"))]
    RemotePackages {
        source: RemoteDebPackageEntriesFromPackagesError,
    },
    #[snafu(display("failed to read remote deb package payload keys"))]
    RemoteKeys {
        source: RemoteDebPackageEntriesFromKeysError,
    },
    #[snafu(display("failed to select publishable deb payloads"))]
    CompareVersions { source: E },
}

pub fn retained_remote_linux_package_payloads(
    manifest: &PackageManifest,
    remote_payloads: Vec<LinuxPackagePayload>,
) -> Result<Vec<LinuxPackagePayload>, RetainedRemoteLinuxPackagePayloadsError> {
    linux_package_payloads_from_manifest(manifest)
        .context(retained_remote_linux_package_payloads_error::LocalPayloadsSnafu)?;
    Ok(remote_payloads)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RetainedRemoteLinuxPackagePayloadsError {
    #[snafu(display("failed to read linux package payloads from manifest"))]
    LocalPayloads {
        source: LinuxPackagePayloadsFromManifestError,
    },
}

pub fn retained_remote_deb_package_entries(
    manifest: &PackageManifest,
    remote_entries: Vec<RemoteDebPackageEntry>,
) -> Result<Vec<RemoteDebPackageEntry>, RetainedRemoteDebPackageEntriesError> {
    ensure!(
        manifest.kind == PackageSystem::Deb,
        retained_remote_deb_package_entries_error::WrongKindSnafu {
            system: manifest.kind
        }
    );
    linux_package_payloads_from_manifest(manifest)
        .context(retained_remote_deb_package_entries_error::LocalPayloadsSnafu)?;
    Ok(remote_entries)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RetainedRemoteDebPackageEntriesError {
    #[snafu(display("{system} package manifest is not a deb package manifest"))]
    WrongKind { system: PackageSystem },
    #[snafu(display("failed to read deb package payloads from manifest"))]
    LocalPayloads {
        source: LinuxPackagePayloadsFromManifestError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutableEntryNames {
    pub latest: String,
    pub versioned: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutableEntryRemoteKeys {
    pub latest: String,
    pub versioned: String,
}

pub fn mutable_entry_names(
    package_id: &PackageId,
    system: PackageSystem,
    source_version: &Version,
) -> Result<MutableEntryNames, MutableEntryNamesError> {
    let extension = match system {
        PackageSystem::Brew => "rb",
        PackageSystem::Scoop => "json",
        PackageSystem::Deb | PackageSystem::Rpm => {
            return Err(MutableEntryNamesError::UnsupportedSystem { system });
        }
    };
    Ok(MutableEntryNames {
        latest: format!("{}.{extension}", package_id.as_str()),
        versioned: format!("{}-{source_version}.{extension}", package_id.as_str()),
    })
}

pub fn mutable_entry_remote_keys(
    prefix: &RemotePrefix,
    package_id: &PackageId,
    system: PackageSystem,
    source_version: &Version,
) -> Result<MutableEntryRemoteKeys, MutableEntryRemoteKeysError> {
    let names = mutable_entry_names(package_id, system, source_version)
        .context(mutable_entry_remote_keys_error::EntryNamesSnafu)?;
    Ok(MutableEntryRemoteKeys {
        latest: prefix.join(&names.latest),
        versioned: prefix.join(&names.versioned),
    })
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum MutableEntryNamesError {
    #[snafu(display("{system} does not define per-package mutable entry files"))]
    UnsupportedSystem { system: PackageSystem },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum MutableEntryRemoteKeysError {
    #[snafu(display("failed to resolve mutable entry names"))]
    EntryNames { source: MutableEntryNamesError },
}

pub fn publish_upload_order(entry: bool) -> u8 {
    if entry { 1 } else { 0 }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishUploadPlan {
    pub key: String,
    pub entry: bool,
    pub condition: Option<UploadCondition>,
}

pub fn apply_mutable_entry_conditions(
    mut uploads: Vec<PublishUploadPlan>,
    conditions: &BTreeMap<String, UploadCondition>,
) -> Result<Vec<PublishUploadPlan>, ApplyMutableEntryConditionsError> {
    for upload in uploads.iter_mut().filter(|upload| upload.entry) {
        upload.condition = Some(conditions.get(&upload.key).cloned().context(
            apply_mutable_entry_conditions_error::MissingBaselineSnafu { key: &upload.key },
        )?);
    }
    Ok(uploads)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ApplyMutableEntryConditionsError {
    #[snafu(display("mutable entry upload {key} is missing remote baseline condition"))]
    MissingBaseline { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3CommonPublishTarget {
    pub bucket: String,
    pub endpoint_url: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3PublishTarget {
    Brew(S3BrewPublishTarget),
    Scoop(S3ScoopPublishTarget),
    Deb(S3DebPublishTarget),
    Rpm(S3RpmPublishTarget),
}

impl S3PublishTarget {
    pub fn common(&self) -> &S3CommonPublishTarget {
        match self {
            Self::Brew(target) => &target.common,
            Self::Scoop(target) => &target.common,
            Self::Deb(target) => &target.common,
            Self::Rpm(target) => &target.common,
        }
    }

    pub fn bucket(&self) -> &str {
        &self.common().bucket
    }

    pub fn endpoint_url(&self) -> &str {
        &self.common().endpoint_url
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3BrewPublishTarget {
    pub common: S3CommonPublishTarget,
    pub prefix: RemotePrefix,
    pub public_base_url: PublicBaseUrl,
    pub tap: TapPublishTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitIndexPublishTarget {
    pub repository: String,
    pub base_branch: String,
    pub token: String,
}

pub type TapPublishTarget = GitIndexPublishTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ScoopPublishTarget {
    pub common: S3CommonPublishTarget,
    pub prefix: RemotePrefix,
    pub public_base_url: PublicBaseUrl,
    pub bucket: GitIndexPublishTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3DebPublishTarget {
    pub common: S3CommonPublishTarget,
    pub prefix: RemotePrefix,
    pub suite: String,
    pub signing_key: String,
    pub signing_passphrase: Option<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3RpmPublishTarget {
    pub common: S3CommonPublishTarget,
    pub prefix: RemotePrefix,
}

pub fn s3_publish_env_names(
    contract: &ReleaseContract,
    system: PackageSystem,
) -> Result<BTreeSet<String>, S3PublishEnvNamesError> {
    let mut names = BTreeSet::new();
    collect_env_ref(&mut names, &contract.destination.s3.endpoint);
    collect_env_ref(&mut names, &contract.destination.s3.access_key_id);
    collect_env_ref(&mut names, &contract.destination.s3.secret_access_key);

    match system {
        PackageSystem::Brew => {
            let branch = contract
                .destination
                .s3
                .brew
                .as_ref()
                .ok_or(S3PublishEnvNamesError::MissingBranch { system })?;
            collect_env_ref(&mut names, &branch.stable.tap.token);
            collect_env_ref(&mut names, &branch.preview.tap.token);
        }
        PackageSystem::Scoop => {
            let branch = contract
                .destination
                .s3
                .scoop
                .as_ref()
                .ok_or(S3PublishEnvNamesError::MissingBranch { system })?;
            collect_env_ref(&mut names, &branch.stable.bucket.token);
            collect_env_ref(&mut names, &branch.preview.bucket.token);
        }
        PackageSystem::Deb => {
            let branch = contract
                .destination
                .s3
                .deb
                .as_ref()
                .ok_or(S3PublishEnvNamesError::MissingBranch { system })?;
            collect_env_ref(&mut names, &branch.signing.key);
            collect_env_ref(&mut names, &branch.signing.passphrase);
            collect_env_ref(&mut names, &branch.signing.fingerprint);
        }
        PackageSystem::Rpm => {
            contract
                .destination
                .s3
                .rpm
                .as_ref()
                .ok_or(S3PublishEnvNamesError::MissingBranch { system })?;
        }
    }

    Ok(names)
}

fn collect_env_ref(names: &mut BTreeSet<String>, ref_: &EnvRef) {
    names.insert(ref_.env.clone());
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum S3PublishEnvNamesError {
    #[snafu(display("destination s3 {system} branch is missing"))]
    MissingBranch { system: PackageSystem },
}

pub fn resolve_s3_publish_target(
    contract: &ReleaseContract,
    system: PackageSystem,
    channel: ReleaseChannel,
    values: &BTreeMap<String, String>,
) -> Result<S3PublishTarget, ResolveS3PublishTargetError> {
    let common = resolve_s3_common(contract, values)?;
    match system {
        PackageSystem::Brew => {
            let branch = contract.destination.s3.brew.as_ref().ok_or(
                ResolveS3PublishTargetError::MissingBranch {
                    system: PackageSystem::Brew,
                },
            )?;
            let destination = match channel {
                ReleaseChannel::Stable => &branch.stable,
                ReleaseChannel::Preview => &branch.preview,
            };
            Ok(S3PublishTarget::Brew(S3BrewPublishTarget {
                common,
                prefix: RemotePrefix::parse(&destination.prefix)
                    .context(resolve_s3_publish_target_error::RemotePrefixSnafu)?,
                public_base_url: PublicBaseUrl::parse(&destination.public_base_url)
                    .context(resolve_s3_publish_target_error::PublicBaseUrlSnafu)?,
                tap: GitIndexPublishTarget {
                    repository: destination.tap.repository.clone(),
                    base_branch: destination.tap.base_branch.clone(),
                    token: resolve_env_ref(&destination.tap.token, values)?,
                },
            }))
        }
        PackageSystem::Scoop => {
            let branch = contract.destination.s3.scoop.as_ref().ok_or(
                ResolveS3PublishTargetError::MissingBranch {
                    system: PackageSystem::Scoop,
                },
            )?;
            let destination = match channel {
                ReleaseChannel::Stable => &branch.stable,
                ReleaseChannel::Preview => &branch.preview,
            };
            Ok(S3PublishTarget::Scoop(S3ScoopPublishTarget {
                common,
                prefix: RemotePrefix::parse(&destination.prefix)
                    .context(resolve_s3_publish_target_error::RemotePrefixSnafu)?,
                public_base_url: PublicBaseUrl::parse(&destination.public_base_url)
                    .context(resolve_s3_publish_target_error::PublicBaseUrlSnafu)?,
                bucket: GitIndexPublishTarget {
                    repository: destination.bucket.repository.clone(),
                    base_branch: destination.bucket.base_branch.clone(),
                    token: resolve_env_ref(&destination.bucket.token, values)?,
                },
            }))
        }
        PackageSystem::Deb => {
            let branch = contract.destination.s3.deb.as_ref().ok_or(
                ResolveS3PublishTargetError::MissingBranch {
                    system: PackageSystem::Deb,
                },
            )?;
            let destination = match channel {
                ReleaseChannel::Stable => &branch.stable,
                ReleaseChannel::Preview => &branch.preview,
            };
            Ok(S3PublishTarget::Deb(S3DebPublishTarget {
                common,
                prefix: RemotePrefix::parse(&destination.prefix)
                    .context(resolve_s3_publish_target_error::RemotePrefixSnafu)?,
                suite: destination.suite.clone(),
                signing_key: resolve_env_ref(&branch.signing.key, values)?,
                signing_passphrase: resolve_optional_env_ref(&branch.signing.passphrase, values)?,
                fingerprint: resolve_env_ref(&branch.signing.fingerprint, values)?,
            }))
        }
        PackageSystem::Rpm => {
            let branch = contract.destination.s3.rpm.as_ref().ok_or(
                ResolveS3PublishTargetError::MissingBranch {
                    system: PackageSystem::Rpm,
                },
            )?;
            let destination = match channel {
                ReleaseChannel::Stable => &branch.stable,
                ReleaseChannel::Preview => &branch.preview,
            };
            Ok(S3PublishTarget::Rpm(S3RpmPublishTarget {
                common,
                prefix: RemotePrefix::parse(&destination.prefix)
                    .context(resolve_s3_publish_target_error::RemotePrefixSnafu)?,
            }))
        }
    }
}

fn resolve_s3_common(
    contract: &ReleaseContract,
    values: &BTreeMap<String, String>,
) -> Result<S3CommonPublishTarget, ResolveS3PublishTargetError> {
    Ok(S3CommonPublishTarget {
        bucket: contract.destination.s3.bucket.clone(),
        endpoint_url: resolve_env_ref(&contract.destination.s3.endpoint, values)?,
        access_key_id: resolve_env_ref(&contract.destination.s3.access_key_id, values)?,
        secret_access_key: resolve_env_ref(&contract.destination.s3.secret_access_key, values)?,
    })
}

fn resolve_env_ref(
    ref_: &EnvRef,
    values: &BTreeMap<String, String>,
) -> Result<String, ResolveS3PublishTargetError> {
    let Some(value) = values.get(&ref_.env) else {
        return Err(ResolveS3PublishTargetError::MissingEnv {
            name: ref_.env.clone(),
        });
    };
    if value.is_empty() {
        return Err(ResolveS3PublishTargetError::EmptyEnv {
            name: ref_.env.clone(),
        });
    }
    Ok(value.clone())
}

fn resolve_optional_env_ref(
    ref_: &EnvRef,
    values: &BTreeMap<String, String>,
) -> Result<Option<String>, ResolveS3PublishTargetError> {
    let Some(value) = values.get(&ref_.env) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(ResolveS3PublishTargetError::EmptyEnv {
            name: ref_.env.clone(),
        });
    }
    Ok(Some(value.clone()))
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ResolveS3PublishTargetError {
    #[snafu(display("destination s3 {system} branch missing"))]
    MissingBranch { system: PackageSystem },
    #[snafu(display("missing required release environment variable {name}"))]
    MissingEnv { name: String },
    #[snafu(display("release environment variable {name} must not be empty"))]
    EmptyEnv { name: String },
    #[snafu(display("invalid remote prefix"))]
    RemotePrefix { source: RemotePrefixError },
    #[snafu(display("invalid public base url"))]
    PublicBaseUrl { source: PublicBaseUrlError },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrefix(String);

impl RemotePrefix {
    pub fn parse(value: &str) -> Result<Self, RemotePrefixError> {
        let trimmed = value.trim_matches('/');
        if trimmed.is_empty() {
            return Err(RemotePrefixError::EmptyPrefix);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, relative: &str) -> String {
        format!("{}/{relative}", self.0)
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RemotePrefixError {
    #[snafu(display("remote prefix must not be empty"))]
    EmptyPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBaseUrl(String);

impl PublicBaseUrl {
    pub fn parse(value: &str) -> Result<Self, PublicBaseUrlError> {
        let trimmed = value.trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(PublicBaseUrlError::EmptyPublicBaseUrl);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, relative: &str) -> String {
        format!("{}/{relative}", self.0)
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PublicBaseUrlError {
    #[snafu(display("public base url must not be empty"))]
    EmptyPublicBaseUrl,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{
        channel::ReleaseChannel,
        contract::ReleaseContract,
        publish::{S3PublishTarget, resolve_s3_publish_target},
        system::PackageSystem,
    };

    fn contract() -> ReleaseContract {
        toml::from_str(
            r#"
                [package.product]
                version = "1.2.3-beta.1"
                description = "product"
                license = "Apache-2.0"
                homepage = "https://example.test"

                [package.product.deb]
                revision = "1"
                architecture = "target"
                dockerfile = "xtask/release/deb/Dockerfile"

                [package.product.rpm]
                release = "1"
                architecture = "target"
                dockerfile = "xtask/release/rpm/Dockerfile"

                [package.product.brew]
                script = "xtask/release/brew/product.sh"
                manifest_template = "xtask/templates/product.rb.in"

                [package.product.scoop]
                script = "xtask/release/scoop/product.sh"
                manifest_template = "xtask/templates/product.json.in"

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
        .expect("contract should parse")
    }

    fn values() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "S3_ENDPOINT".to_string(),
                "https://r2.example.test".to_string(),
            ),
            ("S3_ACCESS_KEY_ID".to_string(), "access".to_string()),
            ("S3_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
            ("APT_SIGNING_KEY".to_string(), "key".to_string()),
            ("APT_SIGNING_PASSPHRASE".to_string(), "pass".to_string()),
            (
                "APT_SIGNING_FINGERPRINT".to_string(),
                "0123456789ABCDEF".to_string(),
            ),
            ("HOMEBREW_TAP_GITHUB_TOKEN".to_string(), "token".to_string()),
        ])
    }

    #[test]
    fn preview_channel_selects_preview_destinations() {
        let contract = contract();
        let values = values();

        let target = resolve_s3_publish_target(
            &contract,
            PackageSystem::Deb,
            ReleaseChannel::Preview,
            &values,
        )
        .expect("deb target should resolve");
        let S3PublishTarget::Deb(target) = target else {
            panic!("expected deb target");
        };
        assert_eq!(target.prefix.as_str(), "ppa/genmeta");
        assert_eq!(target.suite, "preview");

        let target = resolve_s3_publish_target(
            &contract,
            PackageSystem::Rpm,
            ReleaseChannel::Preview,
            &values,
        )
        .expect("rpm target should resolve");
        let S3PublishTarget::Rpm(target) = target else {
            panic!("expected rpm target");
        };
        assert_eq!(target.prefix.as_str(), "rpm/preview");

        let target = resolve_s3_publish_target(
            &contract,
            PackageSystem::Brew,
            ReleaseChannel::Preview,
            &values,
        )
        .expect("brew target should resolve");
        let S3PublishTarget::Brew(target) = target else {
            panic!("expected brew target");
        };
        assert_eq!(target.prefix.as_str(), "homebrew/preview");
        assert_eq!(target.tap.repository, "genmeta/homebrew-preview");

        let target = resolve_s3_publish_target(
            &contract,
            PackageSystem::Scoop,
            ReleaseChannel::Preview,
            &values,
        )
        .expect("scoop target should resolve");
        let S3PublishTarget::Scoop(target) = target else {
            panic!("expected scoop target");
        };
        assert_eq!(target.prefix.as_str(), "scoop/preview");
        assert_eq!(target.bucket.repository, "genmeta/scoop-preview");
    }

    #[test]
    fn stable_channel_selects_stable_destinations() {
        let contract = contract();
        let values = values();

        let target = resolve_s3_publish_target(
            &contract,
            PackageSystem::Brew,
            ReleaseChannel::Stable,
            &values,
        )
        .expect("brew target should resolve");
        let S3PublishTarget::Brew(target) = target else {
            panic!("expected brew target");
        };
        assert_eq!(target.prefix.as_str(), "homebrew/stable");
        assert_eq!(target.tap.repository, "genmeta/homebrew-stable");
    }
}
