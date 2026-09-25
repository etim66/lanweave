//! Release checks and self-updates for installer-managed copies.
//!
//! Only copies installed by the release installer are eligible: the installer
//! writes a receipt that records the install directory, and [`check`] refuses
//! to offer an update when the running executable does not match it. This
//! keeps development builds, `cargo install` copies, and distro packages from
//! replacing themselves.
//!
//! All network and process work happens here, away from the event loop. The
//! application only sees the [`UpdateCheck`] result and the new version string.

use axoupdater::{AxoUpdater, AxoupdateError};

/// The application name used for the install receipt and release assets.
pub(crate) const APP_NAME: &str = "lanweave";

/// The install command shown to copies that cannot update themselves.
///
/// It is split into lines that fit the dialog card; joining the lines with a
/// space restores the single command.
pub(crate) const INSTALL_COMMAND_LINES: [&str; 3] = [
    "curl --proto '=https' --tlsv1.2 -LsSf",
    "https://github.com/etim66/lanweave/releases/latest/download/",
    "lanweave-installer.sh | sh",
];

/// The result of checking for a newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateCheck {
    /// The running version is the newest stable release.
    UpToDate { current: String },
    /// A newer release is available.
    Available { current: String, new: String },
    /// This copy was not installed by the release installer.
    NotManaged,
    /// The check could not be completed.
    Failed(String),
}

/// Checks for a newer stable release without changing anything.
pub(crate) async fn check() -> UpdateCheck {
    let current = env!("CARGO_PKG_VERSION").to_owned();
    let mut updater = configured_updater();

    // A missing or foreign receipt means this copy is not ours to replace.
    if updater.load_receipt().is_err() {
        return UpdateCheck::NotManaged;
    }
    match updater.check_receipt_is_for_this_executable() {
        Ok(true) => {}
        Ok(false) | Err(_) => return UpdateCheck::NotManaged,
    }

    // The new version is queried first so the later need-check reuses the
    // release it found instead of asking GitHub twice.
    let new = match updater.query_new_version().await {
        Ok(Some(version)) => version.to_string(),
        Ok(None) => return UpdateCheck::UpToDate { current },
        Err(error) => return UpdateCheck::Failed(describe(&error)),
    };

    match updater.is_update_needed().await {
        Ok(true) => UpdateCheck::Available { current, new },
        Ok(false) => UpdateCheck::UpToDate { current },
        Err(error) => UpdateCheck::Failed(describe(&error)),
    }
}

/// Downloads and installs the newest release, returning its version.
///
/// The installer runs without any output reaching the terminal, so the TUI is
/// never corrupted; failures leave the running copy unchanged.
pub(crate) async fn apply() -> Result<String, String> {
    let mut updater = configured_updater();
    updater.load_receipt().map_err(|error| describe(&error))?;

    match updater.run().await {
        Ok(Some(result)) => Ok(result.new_version.to_string()),
        Ok(None) => Err("Lanweave is already up to date.".to_owned()),
        Err(error) => Err(describe(&error)),
    }
}

/// Builds an updater with installer output suppressed.
///
/// The installer would otherwise print progress to stdout and stderr, which
/// would draw over the terminal UI. `LANWEAVE_GITHUB_TOKEN` is honoured for
/// users behind rate limits.
fn configured_updater() -> AxoUpdater {
    let mut updater = AxoUpdater::new_for(APP_NAME);
    updater.disable_installer_output();
    if let Ok(token) = std::env::var("LANWEAVE_GITHUB_TOKEN")
        && !token.is_empty()
    {
        updater.set_github_token(&token);
    }
    updater
}

/// Maps an updater error to a short, display-safe explanation.
///
/// Raw error text is never shown: it can contain local paths and URLs that are
/// noise in the terminal.
fn describe(error: &AxoupdateError) -> String {
    let message = match error {
        AxoupdateError::Reqwest(_) | AxoupdateError::UrlParseError(_) => {
            "Could not reach GitHub. Check your network connection and try again."
        }
        AxoupdateError::NoReceipt { .. }
        | AxoupdateError::ReceiptLoadFailed { .. }
        | AxoupdateError::ConfigFetchFailed { .. } => {
            "This copy was not installed by the Lanweave installer."
        }
        AxoupdateError::NoStableReleases { .. }
        | AxoupdateError::ReleaseNotFound { .. }
        | AxoupdateError::VersionNotFound { .. } => "No newer release is available yet.",
        AxoupdateError::NoInstallerForPackage {} => {
            "The latest release has no installer for this platform."
        }
        AxoupdateError::InstallFailed { .. } => {
            "The installer could not complete the update. The current copy is unchanged."
        }
        AxoupdateError::CleanupFailed {} => {
            "The update was installed, but the old copy could not be removed."
        }
        _ => "The update could not be completed.",
    };
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use super::{INSTALL_COMMAND_LINES, UpdateCheck};

    #[test]
    fn the_install_hint_uses_the_stable_latest_url() {
        assert_eq!(
            INSTALL_COMMAND_LINES[0],
            "curl --proto '=https' --tlsv1.2 -LsSf"
        );
        assert_eq!(
            INSTALL_COMMAND_LINES[1],
            "https://github.com/etim66/lanweave/releases/latest/download/"
        );
        assert_eq!(INSTALL_COMMAND_LINES[2], "lanweave-installer.sh | sh");
        assert!(
            INSTALL_COMMAND_LINES.iter().all(|line| line.len() <= 72),
            "every line must fit the dialog card"
        );
    }

    #[test]
    fn check_results_are_distinct_and_carry_versions() {
        assert_ne!(
            UpdateCheck::UpToDate {
                current: "0.1.0".to_owned()
            },
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            }
        );
        assert_ne!(
            UpdateCheck::NotManaged,
            UpdateCheck::UpToDate {
                current: "0.1.0".to_owned()
            }
        );
    }
}
