use std::collections::{BTreeMap, BTreeSet};

use snafu::{Snafu, ensure};

use crate::{
    manifest::PackageManifest,
    package::{PackageId, ResolvedPackageMetadata},
};

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum RenderTemplateError {
    #[snafu(display("template variable {name} is not defined"))]
    MissingVariable { name: String },
    #[snafu(display("template contains unresolved placeholders"))]
    UnresolvedPlaceholder,
}

pub fn render_template(
    template: &str,
    variables: &BTreeMap<String, String>,
) -> Result<String, RenderTemplateError> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(RenderTemplateError::UnresolvedPlaceholder);
        };
        let name = after_start[..end].trim().to_string();
        let value = variables
            .get(&name)
            .ok_or(RenderTemplateError::MissingVariable { name })?;
        output.push_str(value);
        rest = &after_start[end + 2..];
    }
    output.push_str(rest);
    ensure!(
        !output.contains("{{") && !output.contains("}}"),
        render_template_error::UnresolvedPlaceholderSnafu
    );
    Ok(output)
}

pub fn ruby_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn build_feature_variables(manifest: &PackageManifest) -> BTreeMap<String, String> {
    let features = manifest
        .artifacts
        .iter()
        .flat_map(|artifact| artifact.features.iter().cloned())
        .collect::<BTreeSet<_>>();
    let ruby_array = features
        .iter()
        .map(|feature| format!("\"{}\"", ruby_string(feature)))
        .collect::<Vec<_>>()
        .join(", ");

    BTreeMap::from([
        (
            "build.features.csv".to_string(),
            features.iter().cloned().collect::<Vec<_>>().join(","),
        ),
        (
            "build.features.ruby_array".to_string(),
            format!("[{ruby_array}]"),
        ),
    ])
}

pub fn package_template_variables(
    package_id: &PackageId,
    metadata: &ResolvedPackageMetadata,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("package.name".to_string(), ruby_string(package_id.as_str())),
        (
            "package.version".to_string(),
            ruby_string(&metadata.source_version.to_string()),
        ),
        (
            "package.description".to_string(),
            ruby_string(&metadata.description),
        ),
        (
            "package.homepage".to_string(),
            ruby_string(&metadata.homepage),
        ),
        (
            "package.license".to_string(),
            ruby_string(&metadata.license),
        ),
    ])
}
