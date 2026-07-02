use semver::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    Preview,
}

impl ReleaseChannel {
    pub fn from_version(version: &Version) -> Self {
        if version.pre.is_empty() {
            Self::Stable
        } else {
            Self::Preview
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Preview => "preview",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReleaseChannel;
    use semver::Version;

    #[test]
    fn stable_version_selects_stable_channel() {
        let version = Version::parse("0.8.0").expect("version should parse");

        assert_eq!(
            ReleaseChannel::from_version(&version),
            ReleaseChannel::Stable
        );
    }

    #[test]
    fn prerelease_versions_select_preview_channel() {
        for input in ["0.8.0-alpha.1", "0.8.0-beta.1", "0.8.0-rc.1"] {
            let version = Version::parse(input).expect("version should parse");

            assert_eq!(
                ReleaseChannel::from_version(&version),
                ReleaseChannel::Preview
            );
        }
    }

    #[test]
    fn channel_names_are_release_contract_names() {
        assert_eq!(ReleaseChannel::Stable.as_str(), "stable");
        assert_eq!(ReleaseChannel::Preview.as_str(), "preview");
    }
}
