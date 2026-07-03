use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{Region, RequestChecksumCalculation},
    error::SdkError,
    operation::{
        get_object::GetObjectError, head_object::HeadObjectError,
        list_objects_v2::ListObjectsV2Error, put_object::PutObjectError,
    },
    primitives::{ByteStream, ByteStreamError},
};
use sha2::Digest;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use tempfile::TempDir;
use walkdir::WalkDir;

use crate::{
    brew::BrewTemplateVariableError,
    brew::brew_template_variables,
    channel::ReleaseChannel,
    cli::S3PublishCommandRequest,
    contract::ReleaseContract,
    manifest::{
        LoadS3PublishCommandManifestsError, PackageArtifact, PackageManifest,
        load_s3_publish_command_manifests,
    },
    package::{PackageId, ParsePackageIdError, ResolvePackageMetadataError, resolve_metadata},
    publish::{
        ImmutableCollisionError, LinuxPackagePayload, LinuxPayloadKeyError,
        PublishableDebPayloadsFromManifestAndPackagesError,
        PublishableLinuxPayloadsFromManifestAndRemoteKeysError,
        RemoteDebPackageEntriesFromPackagesError, RemoteDebPackageEntry, RemotePayloadState,
        ResolveS3PublishTargetError, RetainedRemoteDebPackageEntriesError,
        RetainedRemoteLinuxPackagePayloadsError, S3BrewPublishTarget, S3CommonPublishTarget,
        S3DebPublishTarget, S3PublishTarget, S3RpmPublishTarget, S3ScoopPublishTarget,
        UploadCondition, linux_payload_key, linux_repository_upload_order, mutable_entry_names,
        plan_immutable_upload, plan_versioned_immutable_payload, publish_upload_order,
        publishable_deb_payloads_from_manifest_and_packages,
        publishable_linux_payloads_from_manifest_and_remote_keys,
        remote_deb_package_entries_from_packages, resolve_s3_publish_target,
        retained_remote_deb_package_entries, retained_remote_linux_package_payloads,
    },
    scoop::RenderScoopJsonError,
    system::PackageSystem,
    template::{RenderTemplateError, render_template},
};

const APT_ARCHES: &[&str] = &["amd64", "arm64", "armhf", "i386"];
const APT_COMPONENTS: &[&str] = &["main", "contrib", "non-free"];

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum S3PublishError {
    #[snafu(display("failed to load publish manifests"))]
    LoadManifests {
        source: LoadS3PublishCommandManifestsError,
    },
    #[snafu(display("failed to resolve s3 publish target"))]
    ResolveTarget { source: ResolveS3PublishTargetError },
    #[snafu(display("invalid package id"))]
    InvalidPackageId { source: ParsePackageIdError },
    #[snafu(display("failed to resolve package metadata"))]
    ResolveMetadata { source: ResolvePackageMetadataError },
    #[snafu(display("failed to build brew template variables"))]
    BuildBrewTemplateVariables { source: BrewTemplateVariableError },
    #[snafu(display("failed to build scoop template variables"))]
    BuildScoopTemplateVariables { source: RenderScoopJsonError },
    #[snafu(display("failed to render package metadata template"))]
    RenderTemplate { source: RenderTemplateError },
    #[snafu(display("failed to resolve mutable entry names"))]
    MutableEntryNames {
        source: crate::publish::MutableEntryNamesError,
    },
    #[snafu(display("failed to create publish output directory"))]
    CreatePublishDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("failed to write publish metadata"))]
    WritePublishMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("package artifact is missing archive name"))]
    MissingArchiveName,
    #[snafu(display("package artifact sha256 does not match manifest"))]
    ArtifactSha256Mismatch { path: String },
    #[snafu(display("package is missing from release contract"))]
    MissingPackage { package: String },
    #[snafu(display("package system branch is missing from release contract"))]
    MissingBranch {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("package system branch is missing manifest_template"))]
    MissingManifestTemplate {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("failed to read manifest template"))]
    ReadManifestTemplate {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("remote Packages was not utf-8"))]
    RemotePackagesUtf8 { source: std::string::FromUtf8Error },
    #[snafu(display("failed to select publishable deb payloads"))]
    SelectPublishableDebPayloads {
        source: PublishableDebPayloadsFromManifestAndPackagesError<CompareDebVersionError>,
    },
    #[snafu(display("failed to parse remote deb package entries"))]
    ParseRemoteDebEntries {
        source: RemoteDebPackageEntriesFromPackagesError,
    },
    #[snafu(display("failed to select retained remote deb packages"))]
    SelectRetainedDebEntries {
        source: RetainedRemoteDebPackageEntriesError,
    },
    #[snafu(display("failed to create temporary repository"))]
    CreateRepository { source: std::io::Error },
    #[snafu(display("failed to create apt secret directory"))]
    CreateAptSecretDir { source: std::io::Error },
    #[snafu(display("failed to write apt signing key"))]
    WriteAptSigningKey { source: std::io::Error },
    #[snafu(display("failed to write apt signing passphrase"))]
    WriteAptSigningPassphrase { source: std::io::Error },
    #[snafu(display("failed to run apt metadata container"))]
    RunAptMetadataContainer { source: std::io::Error },
    #[snafu(display("apt metadata container failed"))]
    AptMetadataContainerFailed,
    #[snafu(display("remote rpm payload key has unexpected layout"))]
    RemoteRpmPayloadKeyLayout,
    #[snafu(display("failed to infer rpm architecture"))]
    InferRpmArchitecture,
    #[snafu(display("failed to select publishable rpm payloads"))]
    SelectPublishableRpmPayloads {
        source: PublishableLinuxPayloadsFromManifestAndRemoteKeysError<std::convert::Infallible>,
    },
    #[snafu(display("failed to select retained remote rpm payloads"))]
    SelectRetainedRpmPayloads {
        source: RetainedRemoteLinuxPackagePayloadsError,
    },
    #[snafu(display("failed to resolve remote rpm payload key"))]
    ResolveRpmPayloadKey { source: LinuxPayloadKeyError },
    #[snafu(display("failed to run rpm metadata container"))]
    RunRpmMetadataContainer { source: std::io::Error },
    #[snafu(display("rpm metadata container failed"))]
    RpmMetadataContainerFailed,
    #[snafu(display("unsupported repository system"))]
    UnsupportedRepositorySystem,
    #[snafu(display("failed to walk repository"))]
    WalkRepository { source: walkdir::Error },
    #[snafu(display("failed to make repository path relative"))]
    RepositoryPathRelative { source: std::path::StripPrefixError },
    #[snafu(display("remote package artifact collision"))]
    ImmutableCollision { source: ImmutableCollisionError },
    #[snafu(display("metadata upload is missing remote baseline"))]
    MissingMetadataUploadBaseline,
    #[snafu(display("failed to create s3 runtime"))]
    CreateS3Runtime { source: std::io::Error },
    #[snafu(display("failed to read upload body"))]
    ReadUploadBody {
        path: PathBuf,
        source: ByteStreamError,
    },
    #[snafu(display("failed to read remote object body"))]
    ReadRemoteObjectBody {
        key: String,
        source: ByteStreamError,
    },
    #[snafu(display("failed to read remote artifact body"))]
    ReadRemoteArtifactBody {
        key: String,
        source: ByteStreamError,
    },
    #[snafu(display("remote object changed during conditional upload"))]
    ConditionalUploadChanged { key: String },
    #[snafu(display("failed to upload remote object"))]
    UploadObject {
        key: String,
        source: Box<SdkError<PutObjectError>>,
    },
    #[snafu(display("failed to fetch remote object"))]
    FetchObject {
        key: String,
        source: Box<SdkError<GetObjectError>>,
    },
    #[snafu(display("remote object is missing"))]
    MissingRemoteObject { key: String },
    #[snafu(display("failed to create download directory"))]
    CreateDownloadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("failed to write downloaded object"))]
    WriteDownloadedObject {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("failed to list s3 prefix"))]
    ListObjects {
        prefix: String,
        source: Box<SdkError<ListObjectsV2Error>>,
    },
    #[snafu(display("failed to inspect remote object"))]
    InspectObject {
        key: String,
        source: Box<SdkError<HeadObjectError>>,
    },
    #[snafu(display("remote object is missing ETag"))]
    MissingRemoteEtag { key: String },
    #[snafu(display("failed to read file for sha256"))]
    ReadSha256File {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("failed to create copy destination"))]
    CreateCopyDestination {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("failed to copy repository payload"))]
    CopyRepositoryPayload { source: std::io::Error },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum CompareDebVersionError {
    #[snafu(display("failed to parse deb version"))]
    Parse { source: debversion::ParseError },
}

#[derive(Debug, Clone)]
struct S3Options {
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct PlannedUpload {
    path: PathBuf,
    key: String,
    entry: bool,
    condition: Option<UploadCondition>,
}

struct S3Client {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

struct S3PublishContext<'a> {
    root: &'a Path,
    target_dir: &'a Path,
    contract: &'a ReleaseContract,
    options: &'a S3Options,
    client: &'a S3Client,
}

pub fn run(
    root: &Path,
    target_dir: &Path,
    contract: &ReleaseContract,
    command: &S3PublishCommandRequest,
) -> Result<(), S3PublishError> {
    run_inner(root, target_dir, contract, command)
}

fn run_inner(
    root: &Path,
    target_dir: &Path,
    contract: &ReleaseContract,
    command: &S3PublishCommandRequest,
) -> Result<(), S3PublishError> {
    let manifests = load_s3_publish_command_manifests(target_dir, contract, command)
        .context(s3_publish_error::LoadManifestsSnafu)?;
    let values = std::env::vars().collect::<BTreeMap<_, _>>();
    let options = S3Options {
        dry_run: command.dry_run,
    };
    for manifest in manifests {
        let (package_id, metadata) = manifest_package_metadata(root, contract, &manifest)?;
        let channel = ReleaseChannel::from_version(&metadata.source_version);
        let target = resolve_s3_publish_target(contract, manifest.kind, channel, &values)
            .context(s3_publish_error::ResolveTargetSnafu)?;
        eprintln!(
            "publish s3 {} channel={} target={}",
            manifest.kind,
            channel.as_str(),
            target_summary(&target)
        );
        let client = s3_client(target.common())?;
        let publish_context = S3PublishContext {
            root,
            target_dir,
            contract,
            options: &options,
            client: &client,
        };
        match target {
            S3PublishTarget::Brew(target) => {
                publish_brew(&publish_context, target, package_id, metadata, manifest)?;
            }
            S3PublishTarget::Scoop(target) => {
                publish_scoop(&publish_context, target, package_id, metadata, manifest)?;
            }
            S3PublishTarget::Deb(target) => {
                publish_deb(target_dir, &options, &client, target, manifest)?;
            }
            S3PublishTarget::Rpm(target) => {
                publish_rpm(target_dir, &options, &client, target, manifest)?;
            }
        }
    }
    Ok(())
}

fn manifest_package_metadata(
    root: &Path,
    contract: &ReleaseContract,
    manifest: &PackageManifest,
) -> Result<(PackageId, crate::package::ResolvedPackageMetadata), S3PublishError> {
    let package_id = PackageId::new(manifest.package.clone())
        .context(s3_publish_error::InvalidPackageIdSnafu)?;
    let metadata = resolve_metadata(contract, package_id.as_str(), root)
        .context(s3_publish_error::ResolveMetadataSnafu)?;
    Ok((package_id, metadata))
}

fn s3_client(common: &S3CommonPublishTarget) -> Result<S3Client, S3PublishError> {
    let credentials = Credentials::new(
        common.access_key_id.trim().to_string(),
        common.secret_access_key.trim().to_string(),
        None,
        None,
        "genmeta-xtask-release",
    );
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(Region::new("auto"))
        .endpoint_url(common.endpoint_url.to_owned())
        .credentials_provider(credentials)
        .force_path_style(true)
        // Cloudflare R2 rejects the aws-chunked PutObject requests generated by
        // the SDK's default flexible checksum policy.
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .build();
    Ok(S3Client {
        runtime: tokio::runtime::Runtime::new().context(s3_publish_error::CreateS3RuntimeSnafu)?,
        client: Client::from_conf(config),
    })
}

impl S3Client {
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

fn target_summary(target: &S3PublishTarget) -> String {
    match target {
        S3PublishTarget::Brew(target) => {
            format!(
                "prefix={} tap={}",
                target.prefix.as_str(),
                target.tap.repository
            )
        }
        S3PublishTarget::Scoop(target) => format!(
            "prefix={} bucket={}",
            target.prefix.as_str(),
            target.bucket.repository
        ),
        S3PublishTarget::Deb(target) => {
            format!("prefix={} suite={}", target.prefix.as_str(), target.suite)
        }
        S3PublishTarget::Rpm(target) => format!("prefix={}", target.prefix.as_str()),
    }
}

fn publish_brew(
    context: &S3PublishContext<'_>,
    target: S3BrewPublishTarget,
    package_id: PackageId,
    metadata: crate::package::ResolvedPackageMetadata,
    manifest: PackageManifest,
) -> Result<(), S3PublishError> {
    let (mut uploads, manifest) = plan_versioned_archive_uploads(
        context.target_dir,
        context.client,
        target.common.bucket.as_str(),
        &target.prefix,
        manifest,
    )?;
    let template = manifest_template(
        context.root,
        context.contract,
        &package_id,
        PackageSystem::Brew,
    )?;
    let variables =
        brew_template_variables(&package_id, &metadata, &manifest, &target.public_base_url)
            .context(s3_publish_error::BuildBrewTemplateVariablesSnafu)?;
    let formula =
        render_template(&template, &variables).context(s3_publish_error::RenderTemplateSnafu)?;
    let names = mutable_entry_names(&package_id, PackageSystem::Brew, &metadata.source_version)
        .context(s3_publish_error::MutableEntryNamesSnafu)?;
    let out_dir = context.target_dir.join("common").join("brew");
    fs::create_dir_all(&out_dir).context(s3_publish_error::CreatePublishDirSnafu {
        path: out_dir.clone(),
    })?;
    let latest = out_dir.join(&names.latest);
    let versioned = out_dir.join(&names.versioned);
    fs::write(&latest, &formula).context(s3_publish_error::WritePublishMetadataSnafu {
        path: latest.clone(),
    })?;
    fs::write(&versioned, formula).context(s3_publish_error::WritePublishMetadataSnafu {
        path: versioned.clone(),
    })?;
    uploads.push(PlannedUpload {
        path: latest,
        key: target.prefix.join(&names.latest),
        entry: true,
        condition: None,
    });
    uploads.push(PlannedUpload {
        path: versioned,
        key: target.prefix.join(&names.versioned),
        entry: true,
        condition: None,
    });
    publish_uploads(
        context.options,
        context.client,
        target.common.bucket.as_str(),
        uploads,
    )
}

fn publish_scoop(
    context: &S3PublishContext<'_>,
    target: S3ScoopPublishTarget,
    package_id: PackageId,
    metadata: crate::package::ResolvedPackageMetadata,
    manifest: PackageManifest,
) -> Result<(), S3PublishError> {
    let (mut uploads, manifest) = plan_versioned_archive_uploads(
        context.target_dir,
        context.client,
        target.common.bucket.as_str(),
        &target.prefix,
        manifest,
    )?;
    let template = manifest_template(
        context.root,
        context.contract,
        &package_id,
        PackageSystem::Scoop,
    )?;
    let variables = crate::scoop::scoop_template_variables(
        &package_id,
        &metadata,
        &manifest,
        &target.public_base_url,
        &[],
    )
    .context(s3_publish_error::BuildScoopTemplateVariablesSnafu)?;
    let json =
        render_template(&template, &variables).context(s3_publish_error::RenderTemplateSnafu)?;
    let names = mutable_entry_names(&package_id, PackageSystem::Scoop, &metadata.source_version)
        .context(s3_publish_error::MutableEntryNamesSnafu)?;
    let out_dir = context.target_dir.join("common").join("scoop");
    fs::create_dir_all(&out_dir).context(s3_publish_error::CreatePublishDirSnafu {
        path: out_dir.clone(),
    })?;
    let latest = out_dir.join(&names.latest);
    let versioned = out_dir.join(&names.versioned);
    fs::write(&latest, &json).context(s3_publish_error::WritePublishMetadataSnafu {
        path: latest.clone(),
    })?;
    fs::write(&versioned, json).context(s3_publish_error::WritePublishMetadataSnafu {
        path: versioned.clone(),
    })?;
    uploads.push(PlannedUpload {
        path: latest,
        key: target.prefix.join(&names.latest),
        entry: true,
        condition: None,
    });
    uploads.push(PlannedUpload {
        path: versioned,
        key: target.prefix.join(&names.versioned),
        entry: true,
        condition: None,
    });
    publish_uploads(
        context.options,
        context.client,
        target.common.bucket.as_str(),
        uploads,
    )
}

fn plan_versioned_archive_uploads(
    target_dir: &Path,
    client: &S3Client,
    bucket: &str,
    prefix: &crate::publish::RemotePrefix,
    mut manifest: PackageManifest,
) -> Result<(Vec<PlannedUpload>, PackageManifest), S3PublishError> {
    let mut uploads = Vec::new();
    for artifact in &mut manifest.artifacts {
        let archive_name = artifact
            .archive_name
            .clone()
            .context(s3_publish_error::MissingArchiveNameSnafu)?;
        let path = artifact_path(target_dir, artifact);
        let actual_sha256 = sha256_file(&path)?;
        ensure!(
            actual_sha256 == artifact.sha256,
            s3_publish_error::ArtifactSha256MismatchSnafu {
                path: artifact.path.clone(),
            }
        );
        let key = prefix.join(&archive_name);
        let remote = remote_artifact_state(client, bucket, &key)?;
        let plan = plan_versioned_immutable_payload(&key, &actual_sha256, remote);
        artifact.sha256 = plan.metadata_sha256().to_string();
        if let Some(condition) = plan.upload_condition() {
            uploads.push(PlannedUpload {
                path,
                key,
                entry: false,
                condition: Some(condition),
            });
        } else if plan.remote_sha256_matches_local() {
            eprintln!("remote immutable package artifact already has matching sha256: {key}");
        } else {
            eprintln!(
                "remote immutable package artifact already exists with different sha256; reusing remote payload for metadata: {key}"
            );
        }
    }
    Ok((uploads, manifest))
}

fn manifest_template(
    root: &Path,
    contract: &ReleaseContract,
    package_id: &PackageId,
    system: PackageSystem,
) -> Result<String, S3PublishError> {
    let package =
        contract
            .package(package_id.as_str())
            .context(s3_publish_error::MissingPackageSnafu {
                package: package_id.as_str().to_string(),
            })?;
    let branch = package
        .branch(system)
        .context(s3_publish_error::MissingBranchSnafu {
            package: package_id.as_str().to_string(),
            system,
        })?;
    let template =
        branch
            .manifest_template()
            .context(s3_publish_error::MissingManifestTemplateSnafu {
                package: package_id.as_str().to_string(),
                system,
            })?;
    let path = root.join(template);
    fs::read_to_string(&path).context(s3_publish_error::ReadManifestTemplateSnafu { path })
}

fn publish_deb(
    target_dir: &Path,
    options: &S3Options,
    client: &S3Client,
    target: S3DebPublishTarget,
    manifest: PackageManifest,
) -> Result<(), S3PublishError> {
    let remote_packages = remote_deb_packages(client, target.common.bucket.as_str(), &target)?;
    let local_payloads = publishable_deb_payloads_from_manifest_and_packages(
        &manifest,
        remote_packages.iter().map(String::as_str),
        compare_deb_versions,
    )
    .context(s3_publish_error::SelectPublishableDebPayloadsSnafu)?;
    let remote_entries = remote_packages
        .iter()
        .map(|content| remote_deb_package_entries_from_packages(content))
        .collect::<Result<Vec<_>, _>>()
        .context(s3_publish_error::ParseRemoteDebEntriesSnafu)?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let retained = retained_remote_deb_package_entries(&manifest, remote_entries)
        .context(s3_publish_error::SelectRetainedDebEntriesSnafu)?;
    let conditions = remote_deb_entry_conditions(client, target.common.bucket.as_str(), &target)?;
    let repository = build_deb_repository(
        target_dir,
        client,
        target.common.bucket.as_str(),
        &target,
        local_payloads,
        retained,
    )?;
    generate_deb_metadata(repository.path(), &target)?;
    let mut uploads = repository_uploads(repository.path(), &target.prefix, PackageSystem::Deb)?;
    uploads = plan_repository_uploads(client, target.common.bucket.as_str(), uploads)?;
    apply_entry_conditions(&mut uploads, &conditions)?;
    uploads.sort_by(|left, right| {
        linux_repository_upload_order(PackageSystem::Deb, &left.key)
            .unwrap_or(2)
            .cmp(&linux_repository_upload_order(PackageSystem::Deb, &right.key).unwrap_or(2))
            .then_with(|| left.key.cmp(&right.key))
    });
    publish_uploads(options, client, target.common.bucket.as_str(), uploads)
}

fn remote_deb_packages(
    client: &S3Client,
    bucket: &str,
    target: &S3DebPublishTarget,
) -> Result<Vec<String>, S3PublishError> {
    let mut packages = Vec::new();
    for component in APT_COMPONENTS {
        for arch in APT_ARCHES {
            let key = target.prefix.join(&format!(
                "dists/{}/{component}/binary-{arch}/Packages",
                target.suite
            ));
            if let Some(bytes) = get_object_bytes(client, bucket, &key)? {
                packages.push(
                    String::from_utf8(bytes).context(s3_publish_error::RemotePackagesUtf8Snafu)?,
                );
            }
        }
    }
    Ok(packages)
}

fn remote_deb_entry_conditions(
    client: &S3Client,
    bucket: &str,
    target: &S3DebPublishTarget,
) -> Result<BTreeMap<String, UploadCondition>, S3PublishError> {
    let mut conditions = BTreeMap::new();
    for relative in deb_entry_metadata_relative_paths(&target.suite) {
        let key = target.prefix.join(&relative);
        let condition = remote_upload_condition(client, bucket, &key)?;
        conditions.insert(key, condition);
    }
    Ok(conditions)
}

fn deb_entry_metadata_relative_paths(suite: &str) -> Vec<String> {
    let mut paths = vec![
        format!("dists/{suite}/Release"),
        format!("dists/{suite}/Release.gpg"),
        format!("dists/{suite}/InRelease"),
    ];
    for component in APT_COMPONENTS {
        for arch in APT_ARCHES {
            let base = format!("dists/{suite}/{component}/binary-{arch}");
            paths.push(format!("{base}/Packages"));
            paths.push(format!("{base}/Packages.gz"));
            paths.push(format!("{base}/Release"));
        }
    }
    paths
}

fn build_deb_repository(
    target_dir: &Path,
    client: &S3Client,
    bucket: &str,
    target: &S3DebPublishTarget,
    local_payloads: Vec<LinuxPackagePayload>,
    retained_remote: Vec<RemoteDebPackageEntry>,
) -> Result<TempDir, S3PublishError> {
    let repository = tempfile::tempdir().context(s3_publish_error::CreateRepositorySnafu)?;
    for payload in local_payloads {
        let destination = repository
            .path()
            .join(pool_path(&payload.package, &payload.archive_name));
        copy_file(
            &artifact_path_from_manifest(target_dir, &payload.path),
            &destination,
        )?;
    }
    for remote in retained_remote {
        let destination = repository.path().join(&remote.filename);
        let key = target.prefix.join(&remote.filename);
        download_object(client, bucket, &key, &destination)?;
    }
    Ok(repository)
}

fn generate_deb_metadata(
    repository: &Path,
    target: &S3DebPublishTarget,
) -> Result<(), S3PublishError> {
    let secrets = tempfile::tempdir().context(s3_publish_error::CreateAptSecretDirSnafu)?;
    let key_path = secrets.path().join("key.asc");
    let passphrase_path = secrets.path().join("passphrase");
    fs::write(&key_path, &target.signing_key).context(s3_publish_error::WriteAptSigningKeySnafu)?;
    let has_passphrase = if let Some(passphrase) = &target.signing_passphrase {
        fs::write(&passphrase_path, passphrase)
            .context(s3_publish_error::WriteAptSigningPassphraseSnafu)?;
        true
    } else {
        false
    };
    let script = deb_metadata_script(&target.suite, &target.fingerprint, has_passphrase);
    let status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("--volume")
        .arg(format!("{}:/apt-repository", repository.to_string_lossy()))
        .arg("--volume")
        .arg(format!(
            "{}:/apt-secrets:ro",
            secrets.path().to_string_lossy()
        ))
        .arg("debian:bookworm")
        .arg("/bin/sh")
        .arg("-lc")
        .arg(script)
        .status()
        .context(s3_publish_error::RunAptMetadataContainerSnafu)?;
    ensure!(
        status.success(),
        s3_publish_error::AptMetadataContainerFailedSnafu
    );
    Ok(())
}

fn deb_metadata_script(suite: &str, fingerprint: &str, has_passphrase: bool) -> String {
    let architectures = APT_ARCHES.join(" ");
    let components = APT_COMPONENTS.join(" ");
    let mut script = String::from(
        "set -eu\n\
         export DEBIAN_FRONTEND=noninteractive\n\
         apt-get update -qq\n\
         apt-get install --assume-yes -qq dpkg-dev apt-utils gnupg gzip\n\
         cd /apt-repository\n",
    );
    for component in APT_COMPONENTS {
        for arch in APT_ARCHES {
            let base = format!("dists/{suite}/{component}/binary-{arch}");
            script.push_str(&format!(
                "mkdir -p {base}\n\
                 mkdir -p pool/{component}\n\
                 dpkg-scanpackages --multiversion --arch {arch} pool/{component} /dev/null > {base}/Packages\n\
                 gzip -n -c {base}/Packages > {base}/Packages.gz\n\
                 printf 'Archive: {suite}\\nComponent: {component}\\nArchitecture: {arch}\\n' > {base}/Release\n"
            ));
        }
    }
    script.push_str(&format!(
        "mkdir -p dists/{suite}\n\
         apt-ftparchive \
           -o {} \
           -o {} \
           -o {} \
           -o {} \
           -o {} \
           -o {} \
           -o {} \
           -o {} \
           release {} > {}\n",
        shell_quote("APT::FTPArchive::Release::Origin=genmeta"),
        shell_quote("APT::FTPArchive::Release::Label=genmeta"),
        shell_quote("APT::FTPArchive::Release::Suite=stable"),
        shell_quote(&format!("APT::FTPArchive::Release::Codename={suite}")),
        shell_quote("APT::FTPArchive::Release::Version=2025"),
        shell_quote(&format!(
            "APT::FTPArchive::Release::Architectures={architectures}"
        )),
        shell_quote(&format!(
            "APT::FTPArchive::Release::Components={components}"
        )),
        shell_quote("APT::FTPArchive::Release::Description=Genmeta Package Archives"),
        shell_quote(&format!("dists/{suite}")),
        shell_quote(&format!("dists/{suite}/Release")),
    ));
    let fingerprint = normalize_fingerprint(fingerprint);
    let passphrase = if has_passphrase {
        " --passphrase-file /apt-secrets/passphrase"
    } else {
        ""
    };
    script.push_str(&format!(
        "rm -rf /tmp/xtask-apt-gpg\n\
         mkdir -m 700 /tmp/xtask-apt-gpg\n\
         gpg --batch --homedir /tmp/xtask-apt-gpg --import /apt-secrets/key.asc\n\
         actual=\"$(gpg --batch --homedir /tmp/xtask-apt-gpg --with-colons --fingerprint {fingerprint} | awk -F: '$1 == \"fpr\" {{ print toupper($10); exit }}')\"\n\
         if [ \"$actual\" != {quoted_fingerprint} ]; then echo 'gpg fingerprint did not match imported key' >&2; exit 1; fi\n\
         gpg --batch --yes --homedir /tmp/xtask-apt-gpg --pinentry-mode loopback --default-key {fingerprint}{passphrase} --detach-sign --armor -o dists/{suite}/Release.gpg dists/{suite}/Release\n\
         gpg --batch --yes --homedir /tmp/xtask-apt-gpg --pinentry-mode loopback --default-key {fingerprint}{passphrase} --clearsign -o dists/{suite}/InRelease dists/{suite}/Release\n",
        fingerprint = shell_quote(&fingerprint),
        quoted_fingerprint = shell_quote(&fingerprint),
        passphrase = passphrase,
    ));
    script
}

fn publish_rpm(
    target_dir: &Path,
    options: &S3Options,
    client: &S3Client,
    target: S3RpmPublishTarget,
    manifest: PackageManifest,
) -> Result<(), S3PublishError> {
    let remote_keys = list_object_keys(
        client,
        target.common.bucket.as_str(),
        target.prefix.as_str(),
    )?;
    let remote_rpm_keys = remote_keys
        .iter()
        .filter(|key| key.ends_with(".rpm"))
        .map(String::as_str)
        .collect::<Vec<_>>();
    let local_payloads = publishable_linux_payloads_from_manifest_and_remote_keys(
        &manifest,
        &target.prefix,
        remote_rpm_keys.iter().copied(),
        compare_rpm_versions,
    )
    .context(s3_publish_error::SelectPublishableRpmPayloadsSnafu)?;
    let mut retained_remote_payloads = Vec::new();
    for key in remote_rpm_keys {
        retained_remote_payloads.push(remote_rpm_payload_to_linux_payload(&target.prefix, key)?);
    }
    let retained_remote_payloads =
        retained_remote_linux_package_payloads(&manifest, retained_remote_payloads)
            .context(s3_publish_error::SelectRetainedRpmPayloadsSnafu)?;
    let repository = build_rpm_repository(
        target_dir,
        client,
        target.common.bucket.as_str(),
        &target,
        local_payloads,
        retained_remote_payloads,
    )?;
    generate_rpm_metadata(repository.path())?;
    let mut uploads = repository_uploads(repository.path(), &target.prefix, PackageSystem::Rpm)?;
    uploads = plan_repository_uploads(client, target.common.bucket.as_str(), uploads)?;
    uploads.sort_by(|left, right| {
        linux_repository_upload_order(PackageSystem::Rpm, &left.key)
            .unwrap_or(2)
            .cmp(&linux_repository_upload_order(PackageSystem::Rpm, &right.key).unwrap_or(2))
            .then_with(|| left.key.cmp(&right.key))
    });
    publish_uploads(options, client, target.common.bucket.as_str(), uploads)
}

fn remote_rpm_payload_to_linux_payload(
    prefix: &crate::publish::RemotePrefix,
    key: &str,
) -> Result<LinuxPackagePayload, S3PublishError> {
    let relative = key
        .strip_prefix(prefix.as_str())
        .and_then(|value| value.strip_prefix('/'))
        .unwrap_or(key);
    let parts = relative.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() >= 3,
        s3_publish_error::RemoteRpmPayloadKeyLayoutSnafu
    );
    let archive_name = parts[parts.len() - 1].to_string();
    let architecture = archive_name
        .strip_suffix(".rpm")
        .and_then(|stem| stem.rsplit_once('.').map(|(_, arch)| arch.to_string()))
        .context(s3_publish_error::InferRpmArchitectureSnafu)?;
    Ok(LinuxPackagePayload {
        package: parts[0].to_string(),
        version: parts[1].to_string(),
        architecture,
        archive_name,
        path: relative.to_string(),
    })
}

fn build_rpm_repository(
    target_dir: &Path,
    client: &S3Client,
    bucket: &str,
    target: &S3RpmPublishTarget,
    local_payloads: Vec<LinuxPackagePayload>,
    retained_remote: Vec<LinuxPackagePayload>,
) -> Result<TempDir, S3PublishError> {
    let repository = tempfile::tempdir().context(s3_publish_error::CreateRepositorySnafu)?;
    for payload in local_payloads {
        let destination = repository
            .path()
            .join(&payload.package)
            .join(&payload.version)
            .join(&payload.archive_name);
        copy_file(
            &artifact_path_from_manifest(target_dir, &payload.path),
            &destination,
        )?;
    }
    for payload in retained_remote {
        let destination = repository
            .path()
            .join(&payload.package)
            .join(&payload.version)
            .join(&payload.archive_name);
        let key = linux_payload_key(&target.prefix, PackageSystem::Rpm, &payload)
            .context(s3_publish_error::ResolveRpmPayloadKeySnafu)?;
        download_object(client, bucket, &key, &destination)?;
    }
    Ok(repository)
}

fn generate_rpm_metadata(repository: &Path) -> Result<(), S3PublishError> {
    let status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("--volume")
        .arg(format!("{}:/rpm-repository", repository.to_string_lossy()))
        .arg("fedora:40")
        .arg("/bin/sh")
        .arg("-lc")
        .arg(
            "set -euo pipefail; dnf -y -q install createrepo_c; cd /rpm-repository; createrepo_c .",
        )
        .status()
        .context(s3_publish_error::RunRpmMetadataContainerSnafu)?;
    ensure!(
        status.success(),
        s3_publish_error::RpmMetadataContainerFailedSnafu
    );
    Ok(())
}

fn repository_uploads(
    repository: &Path,
    prefix: &crate::publish::RemotePrefix,
    system: PackageSystem,
) -> Result<Vec<PlannedUpload>, S3PublishError> {
    let extension = match system {
        PackageSystem::Deb => "deb",
        PackageSystem::Rpm => "rpm",
        PackageSystem::Brew | PackageSystem::Scoop => {
            return Err(S3PublishError::UnsupportedRepositorySystem);
        }
    };
    let mut uploads = Vec::new();
    for entry in WalkDir::new(repository) {
        let entry = entry.context(s3_publish_error::WalkRepositorySnafu)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(repository)
            .context(s3_publish_error::RepositoryPathRelativeSnafu)?;
        let relative = path_to_slash(relative);
        uploads.push(PlannedUpload {
            path: entry.path().to_path_buf(),
            key: prefix.join(&relative),
            entry: !relative.ends_with(extension),
            condition: None,
        });
    }
    Ok(uploads)
}

fn plan_repository_uploads(
    client: &S3Client,
    bucket: &str,
    uploads: Vec<PlannedUpload>,
) -> Result<Vec<PlannedUpload>, S3PublishError> {
    let mut planned = Vec::new();
    for mut upload in uploads {
        if upload.entry {
            planned.push(upload);
            continue;
        }
        let actual_sha256 = sha256_file(&upload.path)?;
        let remote = remote_artifact_state(client, bucket, &upload.key)?;
        if let Some(condition) = plan_immutable_upload(&upload.key, &actual_sha256, remote)
            .context(s3_publish_error::ImmutableCollisionSnafu)?
        {
            upload.condition = Some(condition);
            planned.push(upload);
        } else {
            eprintln!(
                "remote immutable package artifact already has matching sha256: {}",
                upload.key
            );
        }
    }
    Ok(planned)
}

fn apply_entry_conditions(
    uploads: &mut [PlannedUpload],
    conditions: &BTreeMap<String, UploadCondition>,
) -> Result<(), S3PublishError> {
    for upload in uploads.iter_mut().filter(|upload| upload.entry) {
        upload.condition = Some(
            conditions
                .get(&upload.key)
                .cloned()
                .context(s3_publish_error::MissingMetadataUploadBaselineSnafu)?,
        );
    }
    Ok(())
}

fn publish_uploads(
    options: &S3Options,
    client: &S3Client,
    bucket: &str,
    mut uploads: Vec<PlannedUpload>,
) -> Result<(), S3PublishError> {
    uploads.sort_by(|left, right| {
        publish_upload_order(left.entry)
            .cmp(&publish_upload_order(right.entry))
            .then_with(|| left.key.cmp(&right.key))
    });
    if options.dry_run {
        for upload in uploads {
            eprintln!("would upload {} from {}", upload.key, upload.path.display());
        }
        return Ok(());
    }
    for upload in uploads {
        upload_file(client, bucket, &upload.path, &upload.key, upload.condition)?;
    }
    Ok(())
}

fn upload_file(
    client: &S3Client,
    bucket: &str,
    path: &Path,
    key: &str,
    condition: Option<UploadCondition>,
) -> Result<(), S3PublishError> {
    let body = client.block_on(ByteStream::from_path(path)).context(
        s3_publish_error::ReadUploadBodySnafu {
            path: path.to_path_buf(),
        },
    )?;
    let mut request = client
        .client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body);
    match condition {
        Some(UploadCondition::IfMissing) => {
            request = request.if_none_match("*");
        }
        Some(UploadCondition::IfMatch(etag)) => {
            request = request.if_match(etag);
        }
        None => {}
    }
    match client.block_on(request.send()) {
        Ok(_) => {}
        Err(error) if is_precondition_failed_error(&error) => {
            return Err(S3PublishError::ConditionalUploadChanged {
                key: key.to_string(),
            });
        }
        Err(source) => {
            return Err(S3PublishError::UploadObject {
                key: key.to_string(),
                source: Box::new(source),
            });
        }
    }
    eprintln!("uploaded {key} from {}", path.display());
    Ok(())
}

fn get_object_bytes(
    client: &S3Client,
    bucket: &str,
    key: &str,
) -> Result<Option<Vec<u8>>, S3PublishError> {
    let output = match client.block_on(client.client.get_object().bucket(bucket).key(key).send()) {
        Ok(output) => output,
        Err(error) if is_missing_object_error(&error) => return Ok(None),
        Err(source) => {
            return Err(S3PublishError::FetchObject {
                key: key.to_string(),
                source: Box::new(source),
            });
        }
    };
    let bytes = client
        .block_on(output.body.collect())
        .context(s3_publish_error::ReadRemoteObjectBodySnafu {
            key: key.to_string(),
        })?
        .to_vec();
    Ok(Some(bytes))
}

fn download_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
    path: &Path,
) -> Result<(), S3PublishError> {
    let bytes = get_object_bytes(client, bucket, key)?.context(
        s3_publish_error::MissingRemoteObjectSnafu {
            key: key.to_string(),
        },
    )?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context(s3_publish_error::CreateDownloadDirSnafu {
            path: parent.to_path_buf(),
        })?;
    }
    fs::write(path, bytes).context(s3_publish_error::WriteDownloadedObjectSnafu {
        path: path.to_path_buf(),
    })
}

fn list_object_keys(
    client: &S3Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<String>, S3PublishError> {
    client.block_on(async {
        let mut paginator = client
            .client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .into_paginator()
            .send();
        let mut keys = Vec::new();
        while let Some(page) = paginator.next().await {
            let page = page.map_err(|source| S3PublishError::ListObjects {
                prefix: prefix.to_string(),
                source: Box::new(source),
            })?;
            for object in page.contents() {
                if let Some(key) = object.key() {
                    keys.push(key.to_string());
                }
            }
        }
        Ok(keys)
    })
}

fn remote_artifact_state(
    client: &S3Client,
    bucket: &str,
    key: &str,
) -> Result<RemotePayloadState, S3PublishError> {
    let output = match client.block_on(client.client.get_object().bucket(bucket).key(key).send()) {
        Ok(output) => output,
        Err(error) if is_missing_object_error(&error) => return Ok(RemotePayloadState::Missing),
        Err(source) => {
            return Err(S3PublishError::FetchObject {
                key: key.to_string(),
                source: Box::new(source),
            });
        }
    };
    let sha256 = client.block_on(sha256_stream(output.body, key))?;
    Ok(RemotePayloadState::Present { sha256 })
}

fn remote_upload_condition(
    client: &S3Client,
    bucket: &str,
    key: &str,
) -> Result<UploadCondition, S3PublishError> {
    let output = match client.block_on(client.client.head_object().bucket(bucket).key(key).send()) {
        Ok(output) => output,
        Err(error) if is_missing_head_object_error(&error) => {
            return Ok(UploadCondition::IfMissing);
        }
        Err(source) => {
            return Err(S3PublishError::InspectObject {
                key: key.to_string(),
                source: Box::new(source),
            });
        }
    };
    let etag = output
        .e_tag()
        .context(s3_publish_error::MissingRemoteEtagSnafu {
            key: key.to_string(),
        })?;
    Ok(UploadCondition::IfMatch(etag.to_string()))
}

fn is_missing_object_error(error: &SdkError<GetObjectError, impl std::fmt::Debug>) -> bool {
    if let Some(service) = error.as_service_error() {
        let metadata = service.meta();
        return classify_missing_object(metadata.code(), metadata.message(), None);
    }
    false
}

fn is_missing_head_object_error(error: &SdkError<HeadObjectError, impl std::fmt::Debug>) -> bool {
    if let Some(service) = error.as_service_error() {
        let metadata = service.meta();
        return classify_missing_object(metadata.code(), metadata.message(), None);
    }
    false
}

fn is_precondition_failed_error(error: &SdkError<PutObjectError, impl std::fmt::Debug>) -> bool {
    if let Some(service) = error.as_service_error() {
        let metadata = service.meta();
        return matches!(
            metadata.code(),
            Some("PreconditionFailed") | Some("ConditionalRequestConflict")
        );
    }
    false
}

fn classify_missing_object(code: Option<&str>, message: Option<&str>, status: Option<u16>) -> bool {
    if !matches!(status, None | Some(404)) {
        return false;
    }
    match code {
        Some("NoSuchKey") => true,
        Some("NotFound") => message
            .map(classify_not_found_message_as_object_missing)
            .unwrap_or(true),
        _ => false,
    }
}

fn classify_not_found_message_as_object_missing(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    if message.contains("bucket") {
        return false;
    }
    message.contains("key") || message.contains("object") || message.contains("not found")
}

async fn sha256_stream(mut body: ByteStream, key: &str) -> Result<String, S3PublishError> {
    let mut hasher = sha2::Sha256::new();
    while let Some(bytes) =
        body.next()
            .await
            .transpose()
            .context(s3_publish_error::ReadRemoteArtifactBodySnafu {
                key: key.to_string(),
            })?
    {
        hasher.update(&bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn compare_deb_versions(left: &str, right: &str) -> Result<Ordering, CompareDebVersionError> {
    let left =
        debversion::Version::from_str(left).context(compare_deb_version_error::ParseSnafu)?;
    let right =
        debversion::Version::from_str(right).context(compare_deb_version_error::ParseSnafu)?;
    Ok(left.cmp(&right))
}

fn compare_rpm_versions(left: &str, right: &str) -> Result<Ordering, std::convert::Infallible> {
    Ok(rpm_version::Evr::parse(left).cmp(&rpm_version::Evr::parse(right)))
}

fn artifact_path(target_dir: &Path, artifact: &PackageArtifact) -> PathBuf {
    target_dir.join(&artifact.path)
}

fn artifact_path_from_manifest(target_dir: &Path, path: &str) -> PathBuf {
    target_dir.join(path)
}

fn sha256_file(path: &Path) -> Result<String, S3PublishError> {
    let bytes = fs::read(path).context(s3_publish_error::ReadSha256FileSnafu {
        path: path.to_path_buf(),
    })?;
    Ok(format!("{:x}", sha2::Sha256::digest(&bytes)))
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), S3PublishError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).context(s3_publish_error::CreateCopyDestinationSnafu {
            path: parent.to_path_buf(),
        })?;
    }
    fs::copy(source, destination).context(s3_publish_error::CopyRepositoryPayloadSnafu)?;
    Ok(())
}

fn pool_path(package: &str, filename: &str) -> PathBuf {
    let first = package.chars().next().unwrap_or('_');
    PathBuf::from("pool")
        .join("main")
        .join(first.to_string())
        .join(package)
        .join(filename)
}

fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

fn normalize_fingerprint(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::classify_missing_object;

    #[test]
    fn not_found_without_message_is_missing_object() {
        assert!(classify_missing_object(Some("NotFound"), None, None));
        assert!(classify_missing_object(Some("NotFound"), None, Some(404)));
    }

    #[test]
    fn not_found_bucket_message_is_not_missing_object() {
        assert!(!classify_missing_object(
            Some("NotFound"),
            Some("bucket does not exist"),
            None,
        ));
    }

    #[test]
    fn not_found_non_404_status_is_not_missing_object() {
        assert!(!classify_missing_object(
            Some("NotFound"),
            Some("object not found"),
            Some(403),
        ));
    }
}
