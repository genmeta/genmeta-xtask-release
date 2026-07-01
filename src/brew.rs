use std::collections::BTreeMap;

use snafu::{OptionExt, Snafu, ensure};

use crate::{
    manifest::{PackageArtifact, PackageManifest},
    package::{PackageId, ResolvedPackageMetadata},
    publish::PublicBaseUrl,
    system::PackageSystem,
    template::{build_feature_variables, package_template_variables},
};

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum BrewTemplateVariableError {
    #[snafu(display("brew formula requires brew package manifest"))]
    WrongKind,
    #[snafu(display("brew package artifact is missing archive name"))]
    MissingArchiveName { target: String },
    #[snafu(display("unsupported brew target {target}"))]
    UnsupportedTarget { target: String },
}

pub fn brew_template_variables(
    package_id: &PackageId,
    metadata: &ResolvedPackageMetadata,
    manifest: &PackageManifest,
    public_base_url: &PublicBaseUrl,
) -> Result<BTreeMap<String, String>, BrewTemplateVariableError> {
    ensure!(
        manifest.kind == PackageSystem::Brew,
        brew_template_variable_error::WrongKindSnafu
    );

    let mut variables = package_template_variables(package_id, metadata);
    variables.insert(
        "brew.class".to_string(),
        formula_class_name(package_id.as_str()),
    );
    variables.insert(
        "brew.urls".to_string(),
        formula_urls(manifest, public_base_url)?,
    );
    variables.insert(
        "homebrew.class".to_string(),
        formula_class_name(package_id.as_str()),
    );
    variables.insert(
        "homebrew.urls".to_string(),
        formula_urls(manifest, public_base_url)?,
    );
    variables.insert(
        "homebrew.ssh_session_install".to_string(),
        ssh_session_install(manifest),
    );
    variables.extend(build_feature_variables(manifest));
    Ok(variables)
}

fn ssh_session_install(manifest: &PackageManifest) -> String {
    let has_ssh_session = manifest.artifacts.iter().any(|artifact| {
        artifact
            .features
            .iter()
            .any(|feature| feature == "sshd" || feature == "pam")
    });
    if has_ssh_session {
        "    libexec.install \"pishoo-ssh-session\"\n".to_string()
    } else {
        String::new()
    }
}

fn formula_urls(
    manifest: &PackageManifest,
    public_base_url: &PublicBaseUrl,
) -> Result<String, BrewTemplateVariableError> {
    let mut blocks = Vec::new();
    for artifact in &manifest.artifacts {
        let archive_name = archive_name(artifact)?;
        let block = brew_on_block(&artifact.target)?;
        blocks.push(format!(
            "  {block} do\n    url \"{}\"\n    sha256 \"{}\"\n  end",
            public_base_url.join(archive_name),
            artifact.sha256,
        ));
    }
    Ok(blocks.join("\n\n"))
}

fn archive_name(artifact: &PackageArtifact) -> Result<&str, BrewTemplateVariableError> {
    artifact.archive_name.as_deref().context(
        brew_template_variable_error::MissingArchiveNameSnafu {
            target: artifact.target.clone(),
        },
    )
}

fn brew_on_block(target: &str) -> Result<&'static str, BrewTemplateVariableError> {
    match target {
        "aarch64-apple-darwin" => Ok("on_arm"),
        "x86_64-apple-darwin" => Ok("on_intel"),
        _ => Err(BrewTemplateVariableError::UnsupportedTarget {
            target: target.to_string(),
        }),
    }
}

fn formula_class_name(package: &str) -> String {
    let mut output = String::new();
    let mut uppercase_next = true;
    for c in package.chars() {
        if matches!(c, '-' | '_' | '.') {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            output.extend(c.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(c);
        }
    }
    output
}
