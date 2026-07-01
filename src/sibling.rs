use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use snafu::Snafu;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PatchSource {
    CratesIo,
    Git(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOverride {
    pub source: PatchSource,
    pub package: String,
    pub sibling: String,
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingSource {
    pub name: String,
    pub host_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerMount {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerOverlayPlan {
    pub mounts: Vec<ContainerMount>,
    pub cargo_config_path: PathBuf,
    pub cargo_config: String,
}

pub fn container_overlay_plan(
    siblings: &[SiblingSource],
    overrides: &[PatchOverride],
) -> Result<ContainerOverlayPlan, ContainerOverlayPlanError> {
    let sibling_names = siblings
        .iter()
        .map(|sibling| sibling.name.as_str())
        .collect::<BTreeSet<_>>();
    for override_ in overrides {
        if !sibling_names.contains(override_.sibling.as_str()) {
            return Err(ContainerOverlayPlanError::UnknownSibling {
                sibling: override_.sibling.clone(),
            });
        }
    }

    Ok(ContainerOverlayPlan {
        mounts: siblings
            .iter()
            .map(|sibling| ContainerMount {
                source: sibling.host_path.clone(),
                destination: container_sibling_path(&sibling.name),
                read_only: true,
            })
            .collect(),
        cargo_config_path: PathBuf::from("/opt/cargo/config.toml"),
        cargo_config: render_cargo_patch_config(overrides),
    })
}

fn container_sibling_path(sibling: &str) -> PathBuf {
    PathBuf::from("/sources").join(sibling)
}

pub fn render_cargo_patch_config(overrides: &[PatchOverride]) -> String {
    let mut grouped: BTreeMap<PatchSource, Vec<&PatchOverride>> = BTreeMap::new();
    for override_ in overrides {
        grouped
            .entry(override_.source.clone())
            .or_default()
            .push(override_);
    }

    let mut output = String::new();
    for (source, overrides) in grouped {
        match source {
            PatchSource::CratesIo => output.push_str("[patch.crates-io]\n"),
            PatchSource::Git(url) => output.push_str(&format!("[patch.\"{url}\"]\n")),
        }
        for override_ in overrides {
            output.push_str(&format!(
                "{} = {{ path = \"/sources/{}/{}\" }}\n",
                override_.package,
                override_.sibling,
                override_.relative_path.display()
            ));
        }
        output.push('\n');
    }
    output
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub enum ContainerOverlayPlanError {
    #[snafu(display("sibling source {sibling} is not mounted"))]
    UnknownSibling { sibling: String },
}
