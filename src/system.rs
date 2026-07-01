use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use snafu::Snafu;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSystem {
    Deb,
    Rpm,
    Brew,
    Scoop,
}

impl PackageSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Brew => "brew",
            Self::Scoop => "scoop",
        }
    }
}

impl fmt::Display for PackageSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Snafu)]
#[snafu(display("unknown package system {value}"))]
pub struct ParsePackageSystemError {
    value: String,
}

impl FromStr for PackageSystem {
    type Err = ParsePackageSystemError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "deb" => Ok(Self::Deb),
            "rpm" => Ok(Self::Rpm),
            "brew" => Ok(Self::Brew),
            "scoop" => Ok(Self::Scoop),
            _ => Err(ParsePackageSystemError {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchitectureClass {
    Target,
    All,
    Noarch,
}

impl ArchitectureClass {
    pub fn matches_common_target(self) -> bool {
        matches!(self, Self::All | Self::Noarch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestedTarget {
    Triple(String),
    Common,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Release,
    Debug,
}

impl BuildProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Debug => "debug",
        }
    }
}
