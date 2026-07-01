use std::collections::BTreeMap;

use serde::Serialize;
use snafu::{OptionExt, ResultExt, Snafu, ensure};

use crate::{
    manifest::{PackageArtifact, PackageManifest},
    package::{PackageId, ResolvedPackageMetadata},
    publish::{MutableEntryNamesError, PublicBaseUrl, mutable_entry_names},
    system::PackageSystem,
    template::{build_feature_variables, package_template_variables},
};

#[derive(Debug, Serialize)]
struct ScoopManifest<'a> {
    version: String,
    description: &'a str,
    license: &'a str,
    homepage: &'a str,
    architecture: serde_json::Map<String, serde_json::Value>,
    bin: &'a [String],
    checkver: CheckVer,
    autoupdate: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct CheckVer {
    url: String,
    re: String,
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RenderScoopJsonError {
    #[snafu(display("scoop json requires scoop package manifest"))]
    WrongKind,
    #[snafu(display("scoop branch requires at least one bin entry"))]
    MissingBin,
    #[snafu(display("scoop package artifact is missing archive name"))]
    MissingArchiveName { target: String },
    #[snafu(display("unsupported scoop target {target}"))]
    UnsupportedTarget { target: String },
    #[snafu(display("failed to resolve scoop mutable entry names"))]
    MutableEntryNames { source: MutableEntryNamesError },
    #[snafu(display("failed to serialize scoop json"))]
    Serialize { source: serde_json::Error },
}

pub fn render_scoop_json(
    package_id: &PackageId,
    metadata: &ResolvedPackageMetadata,
    manifest: &PackageManifest,
    public_base_url: &PublicBaseUrl,
    bin: &[String],
) -> Result<String, RenderScoopJsonError> {
    ensure!(
        manifest.kind == PackageSystem::Scoop,
        render_scoop_json_error::WrongKindSnafu
    );
    ensure!(!bin.is_empty(), render_scoop_json_error::MissingBinSnafu);

    let names = mutable_entry_names(package_id, PackageSystem::Scoop, &metadata.source_version)
        .context(render_scoop_json_error::MutableEntryNamesSnafu)?;
    let layout = scoop_layout(manifest, public_base_url)?;

    let scoop_manifest = ScoopManifest {
        version: metadata.source_version.to_string(),
        description: &metadata.description,
        license: &metadata.license,
        homepage: &metadata.homepage,
        architecture: layout.architecture,
        bin,
        checkver: CheckVer {
            url: public_base_url.join(&names.latest),
            re: r#""version"\s*:\s*"([^"]+)""#.to_string(),
        },
        autoupdate: layout.autoupdate,
    };
    serde_json::to_string_pretty(&scoop_manifest)
        .map(|json| json + "\n")
        .context(render_scoop_json_error::SerializeSnafu)
}

pub fn scoop_template_variables(
    package_id: &PackageId,
    metadata: &ResolvedPackageMetadata,
    manifest: &PackageManifest,
    public_base_url: &PublicBaseUrl,
    bin: &[String],
) -> Result<BTreeMap<String, String>, RenderScoopJsonError> {
    ensure!(
        manifest.kind == PackageSystem::Scoop,
        render_scoop_json_error::WrongKindSnafu
    );
    ensure!(!bin.is_empty(), render_scoop_json_error::MissingBinSnafu);

    let layout = scoop_layout(manifest, public_base_url)?;
    let mut variables = package_template_variables(package_id, metadata);
    variables.extend(build_feature_variables(manifest));
    variables.insert(
        "scoop.bin.json".to_string(),
        serde_json::to_string_pretty(bin).context(render_scoop_json_error::SerializeSnafu)?,
    );
    variables.insert(
        "scoop.architecture.json".to_string(),
        serde_json::to_string_pretty(&layout.architecture)
            .context(render_scoop_json_error::SerializeSnafu)?,
    );
    variables.insert(
        "scoop.autoupdate.json".to_string(),
        serde_json::to_string_pretty(&layout.autoupdate)
            .context(render_scoop_json_error::SerializeSnafu)?,
    );
    Ok(variables)
}

struct ScoopLayout {
    architecture: serde_json::Map<String, serde_json::Value>,
    autoupdate: serde_json::Map<String, serde_json::Value>,
}

fn scoop_layout(
    manifest: &PackageManifest,
    public_base_url: &PublicBaseUrl,
) -> Result<ScoopLayout, RenderScoopJsonError> {
    let mut architecture = serde_json::Map::new();
    let mut autoupdate = serde_json::Map::new();
    for artifact in &manifest.artifacts {
        let arch_key = scoop_arch_key(&artifact.target)?;
        let archive_name = archive_name(artifact)?;
        let url = public_base_url.join(archive_name);
        architecture.insert(
            arch_key.to_string(),
            serde_json::json!({
                "url": url,
                "hash": artifact.sha256,
            }),
        );
        autoupdate.insert(
            arch_key.to_string(),
            serde_json::json!({
                "url": public_base_url.join(archive_name),
            }),
        );
    }
    Ok(ScoopLayout {
        architecture,
        autoupdate,
    })
}

fn archive_name(artifact: &PackageArtifact) -> Result<&str, RenderScoopJsonError> {
    artifact
        .archive_name
        .as_deref()
        .context(render_scoop_json_error::MissingArchiveNameSnafu {
            target: artifact.target.clone(),
        })
}

fn scoop_arch_key(target: &str) -> Result<&'static str, RenderScoopJsonError> {
    match target {
        "x86_64-pc-windows-msvc" => Ok("64bit"),
        "i686-pc-windows-msvc" => Ok("32bit"),
        _ => Err(RenderScoopJsonError::UnsupportedTarget {
            target: target.to_string(),
        }),
    }
}
