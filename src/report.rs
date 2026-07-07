use serde::Serialize;

use crate::system::PackageSystem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct S3PublishReport {
    pub schema_version: u32,
    pub dry_run: bool,
    pub manifests: Vec<S3PublishReportManifest>,
}

impl S3PublishReport {
    pub fn new(dry_run: bool) -> Self {
        Self {
            schema_version: 1,
            dry_run,
            manifests: Vec::new(),
        }
    }

    pub fn add_manifest(&mut self, manifest: S3PublishReportManifest) {
        self.manifests.push(manifest);
    }

    pub fn extend_manifests(
        &mut self,
        manifests: impl IntoIterator<Item = S3PublishReportManifest>,
    ) {
        self.manifests.extend(manifests);
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct S3PublishReportManifest {
    pub kind: PackageSystem,
    pub package: String,
    pub version: String,
    pub generated_at: String,
    pub git_commit: Option<String>,
    pub git_dirty: bool,
    pub artifacts: Vec<S3PublishReportArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct S3PublishReportArtifact {
    pub target: String,
    pub path: String,
    pub local_path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_name: Option<String>,
    pub key: String,
}
