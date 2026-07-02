#![allow(clippy::result_large_err)]

use std::{
    collections::BTreeMap,
    env,
    ffi::OsStr,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use semver::Version;
use sha2::Digest;
use snafu::{OptionExt, ResultExt, Snafu};

use crate::{
    cli::{
        ParseXtaskCommandRequestError, PublishCommandRequest, XtaskCommandRequest,
        parse_xtask_command_request_or_exit,
    },
    contract::{PackageBranchRef, ReleaseContract, load_release_contract},
    manifest::{PackageArtifact, PackageManifest, write_package_command_manifest},
    package::{PackageId, PackageVersion, resolve_metadata},
    plan::{
        PlannedPackageExecutor, PlannedPackageUnit, package_command_invocations_with_primary_source,
    },
    requires::{linux_requirement_entries, resolve_requires_for},
    sibling::{ContainerOverlayPlan, SiblingSource},
    system::{BuildProfile, PackageSystem, RequestedTarget},
};

const CONTRACT_PATH: &str = "xtask/release.toml";
const WORKSPACE_CONTAINER_PATH: &str = "/workspace";

pub fn run_current_dir() -> Result<(), RunCurrentDirError> {
    let cwd = env::current_dir().context(run_current_dir_error::CurrentDirSnafu)?;
    let context = RunnerContext::load(cwd)?;
    let command = parse_xtask_command_request_or_exit(&context.contract, env::args_os())
        .context(run_current_dir_error::ParseCommandSnafu)?;
    match command {
        XtaskCommandRequest::Package(command) => run_package_command(&context, command),
        XtaskCommandRequest::Publish(PublishCommandRequest::S3(command)) => {
            run_s3_publish_command(&context, command)
        }
    }
}

struct RunnerContext {
    root: PathBuf,
    contract: ReleaseContract,
    target_dir: PathBuf,
}

impl RunnerContext {
    fn load(root: PathBuf) -> Result<Self, RunCurrentDirError> {
        let contract_path = root.join(CONTRACT_PATH);
        let contract = load_release_contract(&contract_path).context(
            run_current_dir_error::LoadContractSnafu {
                path: contract_path,
            },
        )?;
        let target_dir = cargo_metadata::MetadataCommand::new()
            .current_dir(&root)
            .exec()
            .context(run_current_dir_error::CargoMetadataSnafu)?
            .target_directory
            .into_std_path_buf();
        Ok(Self {
            root,
            contract,
            target_dir,
        })
    }

    fn contract_root(&self) -> &Path {
        &self.root
    }
}

fn run_package_command(
    context: &RunnerContext,
    command: crate::cli::PackageCommandRequest,
) -> Result<(), RunCurrentDirError> {
    let values = env::vars().collect::<BTreeMap<_, _>>();
    let primary = primary_source(&context.root)?;
    let units = package_command_invocations_with_primary_source(
        &context.contract,
        &command,
        primary,
        &values,
    )
    .context(run_current_dir_error::PlanPackageSnafu)?;

    let mut artifacts = BTreeMap::<PackageSystem, Vec<PackageArtifact>>::new();
    for unit in &units {
        let artifact = run_package_unit(context, unit)?;
        artifacts.entry(unit.system).or_default().push(artifact);
    }

    for (system, artifacts) in artifacts {
        let manifest = package_manifest(context, system, artifacts)?;
        write_package_command_manifest(&context.target_dir, &manifest, &context.contract, &command)
            .context(run_current_dir_error::WriteManifestSnafu { system })?;
    }
    Ok(())
}

fn run_s3_publish_command(
    context: &RunnerContext,
    command: crate::cli::S3PublishCommandRequest,
) -> Result<(), RunCurrentDirError> {
    crate::s3::run(
        &context.root,
        &context.target_dir,
        &context.contract,
        &command,
    )
    .context(run_current_dir_error::RunS3PublishSnafu)
}

fn primary_source(root: &Path) -> Result<SiblingSource, RunCurrentDirError> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .context(run_current_dir_error::PrimarySourceNameSnafu)?;
    Ok(SiblingSource {
        name: name.to_string(),
        host_path: root.to_path_buf(),
    })
}

fn run_package_unit(
    context: &RunnerContext,
    unit: &PlannedPackageUnit,
) -> Result<PackageArtifact, RunCurrentDirError> {
    let output_dir = package_output_dir(&context.target_dir, unit);
    if output_dir.exists() {
        remove_artifacts_in(&output_dir, unit.system)?;
    }
    fs::create_dir_all(&output_dir).context(run_current_dir_error::CreateOutputDirSnafu {
        path: output_dir.clone(),
    })?;

    let mut env = unit.invocation.env.clone();
    env.insert(
        "XTASK_RELEASE_CONTRACT_ROOT".to_string(),
        context.root.to_string_lossy().into_owned(),
    );
    env.insert(
        "XTASK_RELEASE_REPO_ROOT".to_string(),
        match &unit.invocation.executor {
            PlannedPackageExecutor::LocalScript { .. } => {
                context.root.to_string_lossy().into_owned()
            }
            PlannedPackageExecutor::DockerImage { .. } => WORKSPACE_CONTAINER_PATH.to_string(),
        },
    );
    env.insert(
        "XTASK_RELEASE_TARGET_DIR".to_string(),
        package_env_target_dir(context, unit),
    );
    env.insert(
        "XTASK_RELEASE_OUT_DIR".to_string(),
        package_env_output_dir(context, unit, &output_dir),
    );
    enrich_package_metadata(context, unit, &mut env)?;
    enrich_linux_requires(context, unit, &mut env)?;

    match &unit.invocation.executor {
        PlannedPackageExecutor::LocalScript { script } => {
            run_local_script(context, script, &env)?;
        }
        PlannedPackageExecutor::DockerImage {
            image,
            dockerfile,
            entrypoint,
        } => {
            run_docker_package(
                context,
                image,
                dockerfile,
                entrypoint,
                &env,
                &unit.invocation.env_mounts,
                unit.source_overlay.as_ref(),
            )?;
        }
    }
    artifact_from_output_dir(unit, &context.target_dir, &output_dir)
}

fn package_output_dir(target_dir: &Path, unit: &PlannedPackageUnit) -> PathBuf {
    match &unit.target {
        RequestedTarget::Common => target_dir.join("common").join(unit.system.as_str()),
        RequestedTarget::Triple(triple) => target_dir
            .join(triple)
            .join(
                unit.invocation
                    .env
                    .get("XTASK_RELEASE_PROFILE")
                    .map(String::as_str)
                    .unwrap_or(BuildProfile::Release.as_str()),
            )
            .join(unit.system.as_str()),
    }
}

fn package_env_target_dir(context: &RunnerContext, unit: &PlannedPackageUnit) -> String {
    match &unit.invocation.executor {
        PlannedPackageExecutor::LocalScript { .. } => {
            context.target_dir.to_string_lossy().into_owned()
        }
        PlannedPackageExecutor::DockerImage { .. } => {
            format!("{WORKSPACE_CONTAINER_PATH}/target")
        }
    }
}

fn package_env_output_dir(
    context: &RunnerContext,
    unit: &PlannedPackageUnit,
    output_dir: &Path,
) -> String {
    match &unit.invocation.executor {
        PlannedPackageExecutor::LocalScript { .. } => output_dir.to_string_lossy().into_owned(),
        PlannedPackageExecutor::DockerImage { .. } => {
            let relative = output_dir
                .strip_prefix(&context.target_dir)
                .unwrap_or(output_dir);
            Path::new(WORKSPACE_CONTAINER_PATH)
                .join("target")
                .join(relative)
                .to_string_lossy()
                .into_owned()
        }
    }
}

fn enrich_package_metadata(
    context: &RunnerContext,
    unit: &PlannedPackageUnit,
    env: &mut BTreeMap<String, String>,
) -> Result<(), RunCurrentDirError> {
    let metadata = resolve_metadata(
        &context.contract,
        unit.package_id.as_str(),
        context.contract_root(),
    )
    .context(run_current_dir_error::ResolveMetadataSnafu {
        package: unit.package_id.clone(),
    })?;
    env.insert(
        "XTASK_RELEASE_SOURCE_VERSION".to_string(),
        metadata.source_version.to_string(),
    );
    env.insert(
        "XTASK_RELEASE_PACKAGE_VERSION".to_string(),
        package_version_for_unit(context, unit, metadata.source_version)?.as_string(),
    );
    Ok(())
}

fn package_version_for_unit(
    context: &RunnerContext,
    unit: &PlannedPackageUnit,
    source: Version,
) -> Result<PackageVersion, RunCurrentDirError> {
    let (_, package) = context
        .contract
        .package_entry(unit.package_id.as_str())
        .ok_or_else(|| RunCurrentDirError::MissingPackage {
            package: unit.package_id.clone(),
        })?;
    let branch =
        package
            .branch(unit.system)
            .ok_or_else(|| RunCurrentDirError::MissingPackageBranch {
                package: unit.package_id.clone(),
                system: unit.system,
            })?;
    match branch {
        PackageBranchRef::Deb(branch) => PackageVersion::deb(source, branch.revision.clone())
            .context(run_current_dir_error::PackageVersionSnafu),
        PackageBranchRef::Rpm(branch) => PackageVersion::rpm(source, branch.release.clone())
            .context(run_current_dir_error::PackageVersionSnafu),
        PackageBranchRef::Brew(_) | PackageBranchRef::Scoop(_) => Ok(PackageVersion::plain(source)),
    }
}

fn enrich_linux_requires(
    context: &RunnerContext,
    unit: &PlannedPackageUnit,
    env: &mut BTreeMap<String, String>,
) -> Result<(), RunCurrentDirError> {
    if !matches!(unit.system, PackageSystem::Deb | PackageSystem::Rpm) {
        return Ok(());
    }
    let requires = resolve_requires_for(
        &context.contract,
        context.contract_root(),
        unit.package_id.as_str(),
        unit.system,
    )
    .context(run_current_dir_error::ResolveRequiresSnafu {
        package: unit.package_id.clone(),
        system: unit.system,
    })?;
    let mut entries = Vec::new();
    for (package, bounds) in requires {
        entries.extend(
            linux_requirement_entries(unit.system, &package, bounds).context(
                run_current_dir_error::RenderRequiresSnafu {
                    package: unit.package_id.clone(),
                    system: unit.system,
                },
            )?,
        );
    }
    env.insert("XTASK_RELEASE_REQUIRES".to_string(), entries.join(", "));
    Ok(())
}

fn run_local_script(
    context: &RunnerContext,
    script: &Path,
    envs: &BTreeMap<String, String>,
) -> Result<(), RunCurrentDirError> {
    let script = context.root.join(script);
    let mut command = Command::new(&script);
    command.current_dir(&context.root).envs(envs);
    run_command(&mut command).context(run_current_dir_error::RunLocalScriptSnafu { script })
}

fn run_docker_package(
    context: &RunnerContext,
    image: &str,
    dockerfile: &Path,
    entrypoint: &str,
    envs: &BTreeMap<String, String>,
    env_mounts: &[crate::plan::PlannedEnvMount],
    overlay: Option<&ContainerOverlayPlan>,
) -> Result<(), RunCurrentDirError> {
    let dockerfile = context.root.join(dockerfile);
    let mut image_key = dockerfile.to_string_lossy().into_owned();
    for (name, value) in envs
        .iter()
        .filter(|(name, _)| name.starts_with("XTASK_RELEASE_"))
    {
        image_key.push_str(name);
        image_key.push('=');
        image_key.push_str(value);
        image_key.push('\n');
    }
    let image = format!("{image}-{}", short_hash(image_key.as_bytes()));
    let mut build = Command::new("docker");
    build
        .current_dir(&context.root)
        .arg("build")
        .arg("-f")
        .arg(&dockerfile)
        .arg("-t")
        .arg(&image);
    for (name, value) in envs
        .iter()
        .filter(|(name, _)| name.starts_with("XTASK_RELEASE_"))
    {
        build.arg("--build-arg").arg(format!("{name}={value}"));
    }
    build.arg(".");
    run_command(&mut build).context(run_current_dir_error::BuildDockerImageSnafu { dockerfile })?;
    let container_cargo_home = container_cargo_home(&image)?;

    let mut run = Command::new("docker");
    run.arg("run").arg("--rm");
    run.arg("--volume").arg(format!(
        "{}:{WORKSPACE_CONTAINER_PATH}",
        context.root.to_string_lossy()
    ));
    mount_cargo_cache(&mut run, &container_cargo_home)?;
    for mount in env_mounts {
        run.arg("--volume").arg(format!(
            "{}:{}{}",
            mount.source.to_string_lossy(),
            mount.destination.to_string_lossy(),
            if mount.read_only { ":ro" } else { "" }
        ));
    }
    if let Some(overlay) = overlay {
        for mount in &overlay.mounts {
            run.arg("--volume").arg(format!(
                "{}:{}{}",
                mount.source.to_string_lossy(),
                mount.destination.to_string_lossy(),
                if mount.read_only { ":ro" } else { "" }
            ));
        }
        let cargo_config = context.target_dir.join(".genmeta-xtask-release-cargo.toml");
        fs::write(&cargo_config, &overlay.cargo_config).context(
            run_current_dir_error::WriteCargoConfigSnafu {
                path: cargo_config.clone(),
            },
        )?;
        run.arg("--volume").arg(format!(
            "{}:{}:ro",
            cargo_config.to_string_lossy(),
            Path::new(&container_cargo_home)
                .join("config.toml")
                .to_string_lossy()
        ));
    }
    for (name, value) in envs {
        run.arg("--env").arg(format!("{name}={value}"));
    }
    run.arg(&image).arg(entrypoint);
    run_command(&mut run).context(run_current_dir_error::RunDockerImageSnafu { image })
}

fn container_cargo_home(image: &str) -> Result<String, RunCurrentDirError> {
    let output = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg("--entrypoint")
        .arg("/bin/sh")
        .arg(image)
        .arg("-lc")
        .arg("printf %s \"${CARGO_HOME:-}\"")
        .output()
        .context(run_current_dir_error::ContainerCargoHomeCommandSnafu {
            image: image.to_string(),
        })?;
    if !output.status.success() {
        return Err(RunCurrentDirError::ContainerCargoHomeStatus {
            image: image.to_string(),
        });
    }
    let cargo_home = String::from_utf8(output.stdout).context(
        run_current_dir_error::ContainerCargoHomeUtf8Snafu {
            image: image.to_string(),
        },
    )?;
    let cargo_home = cargo_home.trim().to_string();
    if cargo_home.is_empty() {
        return Err(RunCurrentDirError::MissingContainerCargoHome {
            image: image.to_string(),
        });
    }
    Ok(cargo_home)
}

fn mount_cargo_cache(
    command: &mut Command,
    container_cargo_home: &str,
) -> Result<(), RunCurrentDirError> {
    let cargo_home = host_cargo_home()?;
    for subdir in ["git", "registry"] {
        let host = Path::new(&cargo_home).join(subdir);
        fs::create_dir_all(&host)
            .context(run_current_dir_error::CreateHostCargoCacheSnafu { path: host.clone() })?;
        command.arg("--volume").arg(format!(
            "{}:{}/{}",
            host.to_string_lossy(),
            container_cargo_home.trim_end_matches('/'),
            subdir
        ));
    }
    Ok(())
}

fn host_cargo_home() -> Result<PathBuf, RunCurrentDirError> {
    if let Some(cargo_home) = env::var_os("CARGO_HOME")
        && !cargo_home.is_empty()
    {
        return Ok(PathBuf::from(cargo_home));
    }
    let home = env::var_os("HOME").ok_or(RunCurrentDirError::MissingHostCargoHome)?;
    if home.is_empty() {
        return Err(RunCurrentDirError::MissingHostCargoHome);
    }
    Ok(PathBuf::from(home).join(".cargo"))
}

fn run_command(command: &mut Command) -> Result<(), RunCommandError> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context(run_command_error::SpawnSnafu)?;
    if !status.success() {
        return Err(RunCommandError::Status { status });
    }
    Ok(())
}

fn remove_artifacts_in(path: &Path, system: PackageSystem) -> Result<(), RunCurrentDirError> {
    for entry in fs::read_dir(path).context(run_current_dir_error::ReadOutputDirSnafu {
        path: path.to_path_buf(),
    })? {
        let entry = entry.context(run_current_dir_error::ReadOutputEntrySnafu {
            path: path.to_path_buf(),
        })?;
        let artifact = entry.path();
        if artifact.extension() == Some(OsStr::new(artifact_extension(system))) {
            fs::remove_file(&artifact)
                .context(run_current_dir_error::RemoveArtifactSnafu { path: artifact })?;
        }
    }
    Ok(())
}

fn artifact_from_output_dir(
    unit: &PlannedPackageUnit,
    target_dir: &Path,
    output_dir: &Path,
) -> Result<PackageArtifact, RunCurrentDirError> {
    let mut artifacts = Vec::new();
    for entry in fs::read_dir(output_dir).context(run_current_dir_error::ReadOutputDirSnafu {
        path: output_dir.to_path_buf(),
    })? {
        let entry = entry.context(run_current_dir_error::ReadOutputEntrySnafu {
            path: output_dir.to_path_buf(),
        })?;
        let path = entry.path();
        if path.extension() == Some(OsStr::new(artifact_extension(unit.system))) {
            artifacts.push(path);
        }
    }
    match artifacts.as_slice() {
        [artifact] => artifact_from_path(unit, target_dir, artifact),
        [] => Err(RunCurrentDirError::MissingArtifact {
            path: output_dir.to_path_buf(),
        }),
        _ => Err(RunCurrentDirError::MultipleArtifacts {
            path: output_dir.to_path_buf(),
        }),
    }
}

fn artifact_from_path(
    unit: &PlannedPackageUnit,
    target_dir: &Path,
    path: &Path,
) -> Result<PackageArtifact, RunCurrentDirError> {
    let package_metadata = match unit.system {
        PackageSystem::Deb => deb_metadata(path)?,
        PackageSystem::Rpm => rpm_metadata(path)?,
        PackageSystem::Brew | PackageSystem::Scoop => PackageFileMetadata::default(),
    };
    let metadata = fs::metadata(path).context(run_current_dir_error::ArtifactMetadataSnafu {
        path: path.to_path_buf(),
    })?;
    let target_relative = path
        .strip_prefix(target_dir)
        .context(run_current_dir_error::ArtifactTargetRelativeSnafu {
            path: path.to_path_buf(),
        })?
        .to_str()
        .context(run_current_dir_error::ArtifactPathUtf8Snafu {
            path: path.to_path_buf(),
        })?
        .to_string();
    Ok(PackageArtifact {
        target: target_value(&unit.target),
        path: target_relative,
        sha256: sha256_file(path)?,
        size: metadata.len(),
        package_name: package_metadata.package_name,
        package_version: package_metadata.package_version,
        architecture: package_metadata.architecture,
        archive_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned),
        features: unit
            .invocation
            .env
            .get("XTASK_RELEASE_FEATURES")
            .into_iter()
            .flat_map(|features| features.split(','))
            .filter(|feature| !feature.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        profile: unit.invocation.env.get("XTASK_RELEASE_PROFILE").cloned(),
    })
}

#[derive(Debug, Default)]
struct PackageFileMetadata {
    package_name: Option<String>,
    package_version: Option<String>,
    architecture: Option<String>,
}

fn deb_metadata(path: &Path) -> Result<PackageFileMetadata, RunCurrentDirError> {
    let output = Command::new("dpkg-deb")
        .arg("--field")
        .arg(path)
        .output()
        .context(run_current_dir_error::DebMetadataCommandSnafu {
            path: path.to_path_buf(),
        })?;
    if !output.status.success() {
        return Err(RunCurrentDirError::DebMetadataStatus {
            path: path.to_path_buf(),
        });
    }
    let stdout =
        String::from_utf8(output.stdout).context(run_current_dir_error::DebMetadataUtf8Snafu {
            path: path.to_path_buf(),
        })?;
    Ok(PackageFileMetadata {
        package_name: stanza_field(&stdout, "Package"),
        package_version: stanza_field(&stdout, "Version"),
        architecture: stanza_field(&stdout, "Architecture"),
    })
}

fn rpm_metadata(path: &Path) -> Result<PackageFileMetadata, RunCurrentDirError> {
    let command = rpm_metadata_command(path)?;
    let output = Command::new(&command.program)
        .args(&command.args)
        .output()
        .context(run_current_dir_error::RpmMetadataCommandSnafu {
            path: path.to_path_buf(),
        })?;
    if !output.status.success() {
        return Err(RunCurrentDirError::RpmMetadataStatus {
            path: path.to_path_buf(),
        });
    }
    let stdout =
        String::from_utf8(output.stdout).context(run_current_dir_error::RpmMetadataUtf8Snafu {
            path: path.to_path_buf(),
        })?;
    let mut lines = stdout.lines();
    Ok(PackageFileMetadata {
        package_name: lines.next().map(ToOwned::to_owned),
        package_version: lines.next().map(ToOwned::to_owned),
        architecture: lines.next().map(ToOwned::to_owned),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RpmMetadataCommand {
    program: String,
    args: Vec<String>,
}

fn rpm_metadata_command(path: &Path) -> Result<RpmMetadataCommand, RunCurrentDirError> {
    let query_args = [
        "-qp".to_string(),
        "--queryformat".to_string(),
        "%{NAME}\n%{VERSION}-%{RELEASE}\n%{ARCH}\n".to_string(),
    ];
    let parent = path
        .parent()
        .context(run_current_dir_error::RpmMetadataParentSnafu {
            path: path.to_path_buf(),
        })?;
    let file_name = path.file_name().and_then(|name| name.to_str()).context(
        run_current_dir_error::RpmMetadataFileNameSnafu {
            path: path.to_path_buf(),
        },
    )?;
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--volume".to_string(),
        format!("{}:/rpm:ro", parent.to_string_lossy()),
        "fedora:40".to_string(),
        "rpm".to_string(),
    ];
    args.extend(query_args);
    args.push(format!("/rpm/{file_name}"));
    Ok(RpmMetadataCommand {
        program: "docker".to_string(),
        args,
    })
}

fn stanza_field(stanza: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    stanza.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .map(str::trim)
            .map(ToOwned::to_owned)
    })
}

fn package_manifest(
    context: &RunnerContext,
    system: PackageSystem,
    mut artifacts: Vec<PackageArtifact>,
) -> Result<PackageManifest, RunCurrentDirError> {
    artifacts.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.package_name.cmp(&right.package_name))
    });
    let package_id = primary_manifest_package(&context.contract)?;
    let metadata = resolve_metadata(
        &context.contract,
        package_id.as_str(),
        context.contract_root(),
    )
    .context(run_current_dir_error::ResolveMetadataSnafu {
        package: package_id.clone(),
    })?;
    Ok(PackageManifest {
        schema_version: 1,
        kind: system,
        package: package_id.as_str().to_string(),
        version: metadata.source_version.to_string(),
        generated_at: generated_at(),
        git_commit: None,
        git_dirty: false,
        artifacts,
    })
}

fn primary_manifest_package(contract: &ReleaseContract) -> Result<PackageId, RunCurrentDirError> {
    contract
        .package
        .iter()
        .find(|(_, package)| package.manifest.is_some())
        .or_else(|| contract.package.iter().next())
        .map(|(id, _)| id.clone())
        .context(run_current_dir_error::EmptyContractSnafu)
}

fn artifact_extension(system: PackageSystem) -> &'static str {
    match system {
        PackageSystem::Deb => "deb",
        PackageSystem::Rpm => "rpm",
        PackageSystem::Brew => "gz",
        PackageSystem::Scoop => "zip",
    }
}

fn target_value(target: &RequestedTarget) -> String {
    match target {
        RequestedTarget::Triple(triple) => triple.clone(),
        RequestedTarget::Common => "common".to_string(),
    }
}

fn generated_at() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn sha256_file(path: &Path) -> Result<String, RunCurrentDirError> {
    let mut file = fs::File::open(path).context(run_current_dir_error::Sha256OpenSnafu {
        path: path.to_path_buf(),
    })?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .context(run_current_dir_error::Sha256ReadSnafu {
                path: path.to_path_buf(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn short_hash(input: &[u8]) -> String {
    let digest = sha2::Sha256::digest(input);
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::rpm_metadata_command;

    #[test]
    fn rpm_metadata_command_uses_fedora_container() {
        let command = rpm_metadata_command(Path::new("/tmp/out/sample.rpm"))
            .expect("container rpm command should resolve");

        assert_eq!(command.program, "docker");
        assert_eq!(command.args[0], "run");
        assert!(command.args.iter().any(|arg| arg == "/tmp/out:/rpm:ro"));
        assert_eq!(
            command.args.last().map(String::as_str),
            Some("/rpm/sample.rpm")
        );
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RunCommandError {
    #[snafu(display("failed to run command"))]
    Spawn { source: std::io::Error },
    #[snafu(display("command exited with {status}"))]
    Status { status: std::process::ExitStatus },
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RunCurrentDirError {
    #[snafu(display("failed to read current directory"))]
    CurrentDir { source: std::io::Error },
    #[snafu(display("failed to load release contract"))]
    LoadContract {
        source: crate::contract::LoadReleaseContractError,
        path: PathBuf,
    },
    #[snafu(display("failed to read cargo metadata"))]
    CargoMetadata { source: cargo_metadata::Error },
    #[snafu(display("failed to parse xtask command"))]
    ParseCommand {
        source: ParseXtaskCommandRequestError,
    },
    #[snafu(display("failed to plan package command"))]
    PlanPackage {
        source: crate::plan::PackageCommandInvocationsError,
    },
    #[snafu(display("primary source directory name is not utf-8"))]
    PrimarySourceName,
    #[snafu(display("failed to create package output directory"))]
    CreateOutputDir {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to resolve package {package} metadata"))]
    ResolveMetadata {
        source: crate::package::ResolvePackageMetadataError,
        package: PackageId,
    },
    #[snafu(display("package {package} does not exist"))]
    MissingPackage { package: PackageId },
    #[snafu(display("package {package} does not define {system} branch"))]
    MissingPackageBranch {
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("failed to compose package version"))]
    PackageVersion {
        source: crate::package::PackageVersionError,
    },
    #[snafu(display("failed to resolve package {package} {system} requirements"))]
    ResolveRequires {
        source: crate::requires::ResolveRequiresError,
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("failed to render package {package} {system} requirements"))]
    RenderRequires {
        source: crate::requires::RenderLinuxRequirementError,
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("failed to run local package script"))]
    RunLocalScript {
        source: RunCommandError,
        script: PathBuf,
    },
    #[snafu(display("failed to build package docker image"))]
    BuildDockerImage {
        source: RunCommandError,
        dockerfile: PathBuf,
    },
    #[snafu(display("failed to read package docker image cargo home"))]
    ContainerCargoHomeCommand {
        source: std::io::Error,
        image: String,
    },
    #[snafu(display("package docker image cargo home command failed"))]
    ContainerCargoHomeStatus { image: String },
    #[snafu(display("package docker image cargo home was not utf-8"))]
    ContainerCargoHomeUtf8 {
        source: std::string::FromUtf8Error,
        image: String,
    },
    #[snafu(display("package docker image does not define CARGO_HOME"))]
    MissingContainerCargoHome { image: String },
    #[snafu(display("host CARGO_HOME is not defined"))]
    MissingHostCargoHome,
    #[snafu(display("failed to create host cargo cache directory"))]
    CreateHostCargoCache {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to write container cargo config"))]
    WriteCargoConfig {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to run package docker image"))]
    RunDockerImage {
        source: RunCommandError,
        image: String,
    },
    #[snafu(display("failed to read package output directory"))]
    ReadOutputDir {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to read package output directory entry"))]
    ReadOutputEntry {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to remove stale package artifact"))]
    RemoveArtifact {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("package script produced no artifact in {path:?}"))]
    MissingArtifact { path: PathBuf },
    #[snafu(display("package script produced multiple artifacts in {path:?}"))]
    MultipleArtifacts { path: PathBuf },
    #[snafu(display("failed to run dpkg-deb metadata command"))]
    DebMetadataCommand {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("dpkg-deb metadata command failed for {path:?}"))]
    DebMetadataStatus { path: PathBuf },
    #[snafu(display("dpkg-deb metadata output was not utf-8"))]
    DebMetadataUtf8 {
        source: std::string::FromUtf8Error,
        path: PathBuf,
    },
    #[snafu(display("failed to run rpm metadata command"))]
    RpmMetadataCommand {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("rpm package artifact path has no parent directory"))]
    RpmMetadataParent { path: PathBuf },
    #[snafu(display("rpm package artifact file name is not valid utf-8"))]
    RpmMetadataFileName { path: PathBuf },
    #[snafu(display("rpm metadata command failed for {path:?}"))]
    RpmMetadataStatus { path: PathBuf },
    #[snafu(display("rpm metadata output was not utf-8"))]
    RpmMetadataUtf8 {
        source: std::string::FromUtf8Error,
        path: PathBuf,
    },
    #[snafu(display("failed to inspect package artifact"))]
    ArtifactMetadata {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to make package artifact path target-relative"))]
    ArtifactTargetRelative {
        source: std::path::StripPrefixError,
        path: PathBuf,
    },
    #[snafu(display("package artifact path is not valid utf-8"))]
    ArtifactPathUtf8 { path: PathBuf },
    #[snafu(display("failed to hash package artifact"))]
    Sha256Open {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("failed to read package artifact for hash"))]
    Sha256Read {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("release contract defines no package"))]
    EmptyContract,
    #[snafu(display("failed to write {system} package manifest"))]
    WriteManifest {
        source: crate::manifest::WritePackageCommandManifestError,
        system: PackageSystem,
    },
    #[snafu(display("failed to run s3 publish command"))]
    RunS3Publish {
        #[snafu(source(from(crate::s3::S3PublishError, Box::new)))]
        source: Box<crate::s3::S3PublishError>,
    },
}
