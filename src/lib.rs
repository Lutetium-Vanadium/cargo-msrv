#![deny(clippy::all)]
#![allow(clippy::upper_case_acronyms)]

use crate::check::{check_toolchain, CheckStatus};
use crate::cli::cmd_matches;
use crate::config::CmdMatches;
use crate::errors::{CargoMSRVError, TResult};
use crate::ui::Printer;
use rust_releases::source::{FetchResources, RustChangelog, Source};
use rust_releases::{semver, Channel, Release};

pub mod check;
pub mod cli;
pub mod command;
pub mod config;
pub mod errors;
pub mod fetch;
pub mod ui;

pub fn run_cargo_msrv() -> TResult<()> {
    let matches = cli::cli().get_matches();
    let config = cmd_matches(&matches)?;

    let index_strategy = RustChangelog::fetch_channel(Channel::Stable)?;
    let index = index_strategy.build_index()?;

    let latest_supported = determine_msrv(&config, &index)?;

    if let MinimalCompatibility::NoCompatibleToolchains = latest_supported {
        Err(CargoMSRVError::UnableToFindAnyGoodVersion {
            command: config.check_command().join(" "),
        })
    } else {
        Ok(())
    }
}

/// An enum to represent the minimal compatibility
#[derive(Clone, Debug)]
pub enum MinimalCompatibility {
    /// A toolchain is compatible, if the outcome of a toolchain check results in a success
    CapableToolchain {
        // toolchain specifier
        toolchain: String,
        // checked Rust version
        version: semver::Version,
    },
    /// Compatibility is none, if the check on the last available toolchain fails
    NoCompatibleToolchains,
}

impl MinimalCompatibility {
    pub fn unwrap_version(&self) -> semver::Version {
        if let Self::CapableToolchain { version, .. } = self {
            return version.clone();
        }

        panic!("Unable to unwrap MinimalCompatibility (CapableToolchain::version)")
    }
}

impl From<CheckStatus> for MinimalCompatibility {
    fn from(from: CheckStatus) -> Self {
        match from {
            CheckStatus::Success { version, toolchain } => {
                MinimalCompatibility::CapableToolchain { version, toolchain }
            }
            CheckStatus::Failure { toolchain: _, .. } => {
                MinimalCompatibility::NoCompatibleToolchains
            }
        }
    }
}

pub fn determine_msrv(
    config: &CmdMatches,
    index: &rust_releases::index::ReleaseIndex,
) -> TResult<MinimalCompatibility> {
    let mut compatibility = MinimalCompatibility::NoCompatibleToolchains;
    let cmd = config.check_command().join(" ");

    let releases = index.releases();
    let ui = Printer::new(releases.len() as u64);
    ui.welcome(config.target(), &cmd);

    // The collecting step is necessary, because Rust can't deal with equal opaque types
    let releases = if config.include_all_patch_releases() {
        index.all_releases_iterator().collect::<Vec<_>>()
    } else {
        index.stable_releases_iterator().collect::<Vec<_>>()
    };

    let included_releases = releases.iter().filter(|release| include_version(release.version(), config.minimum_version(), config.maximum_version()));

    test_against_releases_linearly(
        included_releases,
        &mut compatibility,
        config,
        &ui,
    )?;

    match &compatibility {
        MinimalCompatibility::CapableToolchain {
            toolchain: _,
            version,
        } => {
            ui.finish_with_ok(&version);
        }
        MinimalCompatibility::NoCompatibleToolchains => ui.finish_with_err(&cmd),
    }

    Ok(compatibility)
}

fn test_against_releases_linearly<'release, I>(
    releases: I,
    compatibility: &mut MinimalCompatibility,
    config: &CmdMatches,
    ui: &Printer,
) -> TResult<()>
where
    I: Iterator<Item = &'release &'release Release>
{
    for release in releases {
        ui.show_progress("Checking", release.version());
        let status = check_toolchain(release.version(), config, ui)?;

        if let CheckStatus::Failure { .. } = status {
            break;
        }

        *compatibility = status.into();
    }

    Ok(())
}

fn include_version(current: &semver::Version, min_version: Option<&semver::Version>, max_version: Option<&semver::Version>) -> bool {
    match (min_version, max_version) {
        (Some(min), Some(max)) => current >= min && current <= max,
        (Some(min), None) => current >= min,
        (None, Some(max)) => current <= max,
        (None, None) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_releases::semver::Version;
    use parameterized::{ide, parameterized};

    ide!();

    #[parameterized(current = {
        50, // -inf <= x <= inf
        50, // 1.50.0 <= x <= inf
        50, // -inf <= x <= 1.50.0
        50, // 1.50.0 <= x <= 1.50.0
        50, // 1.49.0 <= x <= 1.50.0
    }, min = {
        None,
        Some(50),
        None,
        Some(50),
        Some(49),
    }, max = {
        None,
        None,
        Some(50),
        Some(50),
        Some(50),
    })]
    fn test_included_versions(current: u64, min: Option<u64>, max: Option<u64>) {
        let current = Version::new(1, current, 0);
        let min_version = min.map(|m| Version::new(1, m, 0));
        let max_version = max.map(|m| Version::new(1, m, 0));

        assert!(include_version(&current, min_version.as_ref(), max_version.as_ref()));
    }

    #[parameterized(current = {
        50, // -inf <= x <= 1.49.0 : false
        50, // 1.51 <= x <= inf    : false
        50, // 1.51 <= x <= 1.52.0 : false
        50, // 1.48 <= x <= 1.49.0 : false
    }, min = {
        None,
        Some(51),
        Some(51),
        Some(48),
    }, max = {
        Some(49),
        None,
        Some(52),
        Some(49),
    })]
    fn test_excluded_versions(current: u64, min: Option<u64>, max: Option<u64>) {
        let current = Version::new(1, current, 0);
        let min_version = min.map(|m| Version::new(1, m, 0));
        let max_version = max.map(|m| Version::new(1, m, 0));

        assert!(!include_version(&current, min_version.as_ref(), max_version.as_ref()));
    }
}
