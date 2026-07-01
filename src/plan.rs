use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use snafu::{ResultExt, Snafu};

use crate::{
    cli::{PackageCommandRequest, S3PublishCommandRequest},
    contract::{PackageBranchRef, ReleaseContract},
    package::PackageId,
    publish::S3PublishTarget,
    sibling::{ContainerOverlayPlan, SiblingSource},
    system::{ArchitectureClass, BuildProfile, PackageSystem, RequestedTarget},
};

pub const PACKAGE_ENTRYPOINT: &str = "/opt/genmeta-release/package";

pub struct PackageSelectionRequest {
    pub system: PackageSystem,
    pub targets: Vec<RequestedTarget>,
    pub features: Vec<String>,
}

pub struct SelectedPackageBranch<'a> {
    pub package_id: &'a PackageId,
    pub system: PackageSystem,
    pub target: RequestedTarget,
    pub branch: PackageBranchRef<'a>,
}

pub fn select_package_branches(
    contract: &ReleaseContract,
    request: PackageSelectionRequest,
) -> Result<Vec<SelectedPackageBranch<'_>>, SelectPackageError> {
    let mut selected = Vec::new();
    for (package_id, package) in &contract.package {
        let Some(branch) = package.branch(request.system) else {
            continue;
        };
        for target in &request.targets {
            if target_matches_branch(target, branch) {
                selected.push(SelectedPackageBranch {
                    package_id,
                    system: request.system,
                    target: target.clone(),
                    branch,
                });
            }
        }
    }

    if selected.is_empty() {
        return Err(SelectPackageError::NoMatchingBranch {
            system: request.system,
        });
    }
    Ok(selected)
}

fn target_matches_branch(target: &RequestedTarget, branch: PackageBranchRef<'_>) -> bool {
    match target {
        RequestedTarget::Common => branch
            .architecture()
            .is_some_and(ArchitectureClass::matches_common_target),
        RequestedTarget::Triple(_) => match branch.architecture() {
            Some(ArchitectureClass::Target) | None => true,
            Some(ArchitectureClass::All | ArchitectureClass::Noarch) => false,
        },
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum SelectPackageError {
    #[snafu(display("no package branch matches requested {system} package target"))]
    NoMatchingBranch { system: PackageSystem },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPackageInvocation {
    pub executor: PlannedPackageExecutor,
    pub env: BTreeMap<String, String>,
    pub env_mounts: Vec<PlannedEnvMount>,
}

impl PlannedPackageInvocation {
    pub fn uses_container(&self) -> bool {
        self.executor.uses_container()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedPackageExecutor {
    LocalScript {
        script: PathBuf,
    },
    DockerImage {
        image: String,
        dockerfile: PathBuf,
        entrypoint: String,
    },
}

impl PlannedPackageExecutor {
    pub fn script(&self) -> Option<&Path> {
        match self {
            Self::LocalScript { script } => Some(script),
            Self::DockerImage { .. } => None,
        }
    }

    pub fn dockerfile(&self) -> Option<&Path> {
        match self {
            Self::LocalScript { .. } => None,
            Self::DockerImage { dockerfile, .. } => Some(dockerfile),
        }
    }

    pub fn entrypoint(&self) -> Option<&str> {
        match self {
            Self::LocalScript { .. } => None,
            Self::DockerImage { entrypoint, .. } => Some(entrypoint),
        }
    }

    pub fn uses_container(&self) -> bool {
        matches!(self, Self::DockerImage { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEnvMount {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPackageUnit {
    pub package_id: PackageId,
    pub system: PackageSystem,
    pub target: RequestedTarget,
    pub invocation: PlannedPackageInvocation,
    pub source_overlay: Option<ContainerOverlayPlan>,
}

pub fn package_command_invocations(
    contract: &ReleaseContract,
    command: &PackageCommandRequest,
    values: &BTreeMap<String, String>,
) -> Result<Vec<PlannedPackageUnit>, PackageCommandInvocationsError> {
    package_command_invocations_inner(contract, command, values, None)
}

pub fn package_command_invocations_with_primary_source(
    contract: &ReleaseContract,
    command: &PackageCommandRequest,
    primary: SiblingSource,
    values: &BTreeMap<String, String>,
) -> Result<Vec<PlannedPackageUnit>, PackageCommandInvocationsError> {
    package_command_invocations_inner(contract, command, values, Some(primary))
}

fn package_command_invocations_inner(
    contract: &ReleaseContract,
    command: &PackageCommandRequest,
    values: &BTreeMap<String, String>,
    primary: Option<SiblingSource>,
) -> Result<Vec<PlannedPackageUnit>, PackageCommandInvocationsError> {
    let mut invocations = Vec::new();
    for build in &command.builds {
        let selected = select_package_branches(
            contract,
            PackageSelectionRequest {
                system: build.system,
                targets: build.args.targets.clone(),
                features: build.args.features.clone(),
            },
        )
        .context(package_command_invocations_error::SelectSnafu {
            system: build.system,
        })?;
        let profile = if build.args.debug {
            BuildProfile::Debug
        } else {
            BuildProfile::Release
        };
        let section_overlay = match &primary {
            Some(primary) => build
                .args
                .container_overlay_plan_with_primary(primary.clone())
                .context(package_command_invocations_error::OverlaySnafu {
                    system: build.system,
                })?,
            None => build.args.container_overlay_plan().context(
                package_command_invocations_error::OverlaySnafu {
                    system: build.system,
                },
            )?,
        };
        for selected_package in selected {
            let invocation = package_invocation_for_profile_with_env_values(
                contract,
                selected_package.package_id.as_str(),
                selected_package.system,
                selected_package.target.clone(),
                profile,
                &build.args.features,
                values,
            )
            .context(package_command_invocations_error::InvocationSnafu {
                package: selected_package.package_id.clone(),
                system: selected_package.system,
            })?;
            let source_overlay = if primary.is_some() && !invocation.uses_container() {
                None
            } else {
                section_overlay.clone()
            };
            invocations.push(PlannedPackageUnit {
                package_id: selected_package.package_id.clone(),
                system: selected_package.system,
                target: selected_package.target,
                invocation,
                source_overlay,
            });
        }
    }
    Ok(invocations)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PackageCommandInvocationsError {
    #[snafu(display("failed to select package units for {system}"))]
    Select {
        source: SelectPackageError,
        system: PackageSystem,
    },
    #[snafu(display("failed to plan {system} package for package {package}"))]
    Invocation {
        source: PlanInvocationError,
        package: PackageId,
        system: PackageSystem,
    },
    #[snafu(display("failed to plan source overlay for {system} package"))]
    Overlay {
        source: crate::cli::PackageSectionOverlayError,
        system: PackageSystem,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedS3Publish {
    pub system: PackageSystem,
    pub dry_run: bool,
    pub target: S3PublishTarget,
    pub invocation: Option<PlannedPackageInvocation>,
}

pub fn s3_publish_command_plan(
    contract: &ReleaseContract,
    command: &S3PublishCommandRequest,
    values: &BTreeMap<String, String>,
) -> Result<Vec<PlannedS3Publish>, S3PublishCommandPlanError> {
    let mut plans = Vec::new();
    for system in &command.systems {
        let target = crate::publish::resolve_s3_publish_target(contract, *system, values)
            .context(s3_publish_command_plan_error::TargetSnafu { system: *system })?;
        plans.push(PlannedS3Publish {
            system: *system,
            dry_run: command.dry_run,
            target,
            invocation: None,
        });
    }
    Ok(plans)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum S3PublishCommandPlanError {
    #[snafu(display("failed to resolve s3 {system} publish target"))]
    Target {
        source: crate::publish::ResolveS3PublishTargetError,
        system: PackageSystem,
    },
}

pub fn package_invocation_for(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
    features: &[String],
) -> Result<PlannedPackageInvocation, PlanInvocationError> {
    package_invocation_for_profile(
        contract,
        package,
        system,
        target,
        BuildProfile::Release,
        features,
    )
}

pub fn package_invocation_for_profile(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
    profile: BuildProfile,
    features: &[String],
) -> Result<PlannedPackageInvocation, PlanInvocationError> {
    let (package_id, package_contract) =
        contract
            .package_entry(package)
            .ok_or_else(|| PlanInvocationError::MissingPackage {
                package: package.to_owned(),
            })?;
    let branch =
        package_contract
            .branch(system)
            .ok_or_else(|| PlanInvocationError::MissingBranch {
                package: package.to_owned(),
                system,
            })?;
    if !target_matches_branch(&target, branch) {
        return Err(PlanInvocationError::TargetNotMatched { system });
    }

    let env = BTreeMap::from([
        ("XTASK_RELEASE_OPERATION".to_string(), "package".to_string()),
        (
            "XTASK_RELEASE_PACKAGE_ID".to_string(),
            package_id.as_str().to_string(),
        ),
        (
            "XTASK_RELEASE_SYSTEM".to_string(),
            system.as_str().to_string(),
        ),
        (
            "XTASK_RELEASE_TARGET".to_string(),
            target_environment_value(&target),
        ),
        (
            "XTASK_RELEASE_PROFILE".to_string(),
            profile.as_str().to_string(),
        ),
        ("XTASK_RELEASE_FEATURES".to_string(), features.join(",")),
    ]);

    Ok(PlannedPackageInvocation {
        executor: package_executor(package_id, system, branch)?,
        env,
        env_mounts: Vec::new(),
    })
}

fn package_executor(
    package: &PackageId,
    system: PackageSystem,
    branch: PackageBranchRef<'_>,
) -> Result<PlannedPackageExecutor, PlanInvocationError> {
    if let Some(script) = branch.script() {
        return Ok(PlannedPackageExecutor::LocalScript {
            script: script.to_path_buf(),
        });
    }
    if let Some(dockerfile) = branch.dockerfile() {
        return Ok(PlannedPackageExecutor::DockerImage {
            image: format!("xtask-release:package-{package}-{system}"),
            dockerfile: dockerfile.to_path_buf(),
            entrypoint: PACKAGE_ENTRYPOINT.to_string(),
        });
    }
    Err(PlanInvocationError::MissingPackageExecutor { system })
}

fn target_environment_value(target: &RequestedTarget) -> String {
    match target {
        RequestedTarget::Triple(triple) => triple.clone(),
        RequestedTarget::Common => "common".to_string(),
    }
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum PlanInvocationError {
    #[snafu(display("package {package} does not exist"))]
    MissingPackage { package: String },
    #[snafu(display("package {package} does not define {system} branch"))]
    MissingBranch {
        package: String,
        system: PackageSystem,
    },
    #[snafu(display("requested target does not match {system} branch"))]
    TargetNotMatched { system: PackageSystem },
    #[snafu(display("package {system} branch missing package executor"))]
    MissingPackageExecutor { system: PackageSystem },
    #[snafu(display("missing required build environment variable {name}"))]
    MissingEnv { name: String },
    #[snafu(display("build environment variable {name} must not be empty"))]
    EmptyEnv { name: String },
    #[snafu(display("build env binding {name} must set exactly one of env or value"))]
    InvalidEnvBinding { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildEnvNames {
    pub required: BTreeSet<String>,
    pub optional: BTreeSet<String>,
}

pub fn build_env_names(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
) -> Result<BuildEnvNames, PlanInvocationError> {
    let (_, package_contract) =
        contract
            .package_entry(package)
            .ok_or_else(|| PlanInvocationError::MissingPackage {
                package: package.to_owned(),
            })?;
    let branch =
        package_contract
            .branch(system)
            .ok_or_else(|| PlanInvocationError::MissingBranch {
                package: package.to_owned(),
                system,
            })?;
    if !target_matches_branch(&target, branch) {
        return Err(PlanInvocationError::TargetNotMatched { system });
    }

    let mut names = BuildEnvNames::default();
    for (name, binding) in &package_contract.build.env {
        insert_env_binding_name(name, binding, &mut names)?;
    }

    if let Some(target_env) = branch.target_env(target_key(&target)) {
        for (name, binding) in target_env {
            if let Some(package_binding) = package_contract.build.env.get(name) {
                remove_env_binding_name(package_binding, &mut names);
            }
            insert_env_binding_name(name, binding, &mut names)?;
        }
    }

    Ok(names)
}

pub fn build_and_s3_publish_env_names(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
) -> Result<BuildEnvNames, BuildAndS3PublishEnvNamesError> {
    let mut names = build_env_names(contract, package, system, target)
        .context(build_and_s3_publish_env_names_error::BuildSnafu)?;
    for name in crate::publish::s3_publish_env_names(contract, system)
        .context(build_and_s3_publish_env_names_error::PublishSnafu)?
    {
        names.optional.remove(&name);
        names.required.insert(name);
    }
    Ok(names)
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum BuildAndS3PublishEnvNamesError {
    #[snafu(display("failed to resolve build environment names"))]
    Build { source: PlanInvocationError },
    #[snafu(display("failed to resolve s3 publish environment names"))]
    Publish {
        source: crate::publish::S3PublishEnvNamesError,
    },
}

fn insert_env_binding_name(
    name: &str,
    binding: &crate::contract::EnvBinding,
    names: &mut BuildEnvNames,
) -> Result<(), PlanInvocationError> {
    match (&binding.env, &binding.value) {
        (Some(env_name), None) => {
            if binding.optional {
                names.required.remove(env_name);
                names.optional.insert(env_name.clone());
            } else {
                names.optional.remove(env_name);
                names.required.insert(env_name.clone());
            }
            Ok(())
        }
        (None, Some(_)) => Ok(()),
        _ => Err(PlanInvocationError::InvalidEnvBinding {
            name: name.to_owned(),
        }),
    }
}

fn remove_env_binding_name(binding: &crate::contract::EnvBinding, names: &mut BuildEnvNames) {
    if let Some(env_name) = &binding.env {
        names.required.remove(env_name);
        names.optional.remove(env_name);
    }
}

pub fn package_invocation_with_env_values(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
    features: &[String],
    values: &BTreeMap<String, String>,
) -> Result<PlannedPackageInvocation, PlanInvocationError> {
    package_invocation_for_profile_with_env_values(
        contract,
        package,
        system,
        target,
        BuildProfile::Release,
        features,
        values,
    )
}

pub fn package_invocation_for_profile_with_env_values(
    contract: &ReleaseContract,
    package: &str,
    system: PackageSystem,
    target: RequestedTarget,
    profile: BuildProfile,
    features: &[String],
    values: &BTreeMap<String, String>,
) -> Result<PlannedPackageInvocation, PlanInvocationError> {
    let (_, package_contract) =
        contract
            .package_entry(package)
            .ok_or_else(|| PlanInvocationError::MissingPackage {
                package: package.to_owned(),
            })?;
    let branch =
        package_contract
            .branch(system)
            .ok_or_else(|| PlanInvocationError::MissingBranch {
                package: package.to_owned(),
                system,
            })?;
    if !target_matches_branch(&target, branch) {
        return Err(PlanInvocationError::TargetNotMatched { system });
    }

    let mut plan = package_invocation_for_profile(
        contract,
        package,
        system,
        target.clone(),
        profile,
        features,
    )?;
    let mut bindings = package_contract
        .build
        .env
        .iter()
        .map(|(name, binding)| (name.as_str(), binding))
        .collect::<BTreeMap<_, _>>();
    if let Some(target_env) = branch.target_env(target_key(&target)) {
        for (name, binding) in target_env {
            bindings.insert(name.as_str(), binding);
        }
    }
    for (name, binding) in bindings {
        if let Some(resolved) = resolve_env_binding(name, binding, values, plan.uses_container())? {
            plan.env.insert(name.to_string(), resolved.value);
            if let Some(mount) = resolved.mount {
                plan.env_mounts.push(mount);
            }
        }
    }
    Ok(plan)
}

struct ResolvedEnvBinding {
    value: String,
    mount: Option<PlannedEnvMount>,
}

fn resolve_env_binding(
    name: &str,
    binding: &crate::contract::EnvBinding,
    values: &BTreeMap<String, String>,
    container: bool,
) -> Result<Option<ResolvedEnvBinding>, PlanInvocationError> {
    match (&binding.env, &binding.value) {
        (Some(env_name), None) => {
            let Some(value) = values.get(env_name) else {
                if binding.optional {
                    return Ok(None);
                }
                return Err(PlanInvocationError::MissingEnv {
                    name: env_name.clone(),
                });
            };
            if value.is_empty() {
                return Err(PlanInvocationError::EmptyEnv {
                    name: env_name.clone(),
                });
            }
            if container && let Some(container_path) = &binding.container_path {
                return Ok(Some(ResolvedEnvBinding {
                    value: container_path.to_string_lossy().into_owned(),
                    mount: Some(PlannedEnvMount {
                        source: PathBuf::from(value),
                        destination: container_path.clone(),
                        read_only: true,
                    }),
                }));
            }
            Ok(Some(ResolvedEnvBinding {
                value: value.clone(),
                mount: None,
            }))
        }
        (None, Some(value)) => {
            if value.is_empty() {
                return Err(PlanInvocationError::EmptyEnv {
                    name: name.to_owned(),
                });
            }
            Ok(Some(ResolvedEnvBinding {
                value: value.clone(),
                mount: None,
            }))
        }
        _ => Err(PlanInvocationError::InvalidEnvBinding {
            name: name.to_owned(),
        }),
    }
}

fn target_key(target: &RequestedTarget) -> &str {
    match target {
        RequestedTarget::Triple(triple) => triple.as_str(),
        RequestedTarget::Common => "common",
    }
}
