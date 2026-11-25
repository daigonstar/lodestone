//! Windows: installing programs (emulators, store launchers) with winget, Windows' own package manager.
//! Only from winget's official source, and only when you press Install. winget downloads each program
//! from its publisher (the packages Lodestone uses were checked to install from the publisher's own site
//! or official GitHub releases) and checks it against the package's SHA-256.
//!
//! Installing accepts the program's licence and winget's source terms, as installing it yourself would;
//! Lodestone says so on the Install button. Some installers ask Windows for permission to install for
//! all users: that's Windows' own prompt.

use crate::model::LaunchSpec;

/// The command that installs one package (run as a background job, with its progress in Downloads).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn install_spec(id: &str) -> LaunchSpec {
    LaunchSpec::new(
        "winget",
        &["install", "--id", id, "--exact", "--source", "winget", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"],
    )
}

/// The command that updates one package to its newest version.
pub fn upgrade_spec(id: &str) -> LaunchSpec {
    LaunchSpec::new(
        "winget",
        &["upgrade", "--id", id, "--exact", "--source", "winget", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"],
    )
}

/// A winget package ID Lodestone passes on: "Publisher.Name" style only.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn valid_id(id: &str) -> bool {
    id.contains('.') && id.len() <= 100 && id.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

#[cfg(test)]
mod tests {
    #[test]
    fn package_ids() {
        assert!(super::valid_id("Libretro.RetroArch"));
        assert!(super::valid_id("xemu-project.xemu"));
        assert!(!super::valid_id("RetroArch"));
        assert!(!super::valid_id("a.b; rm -rf"));
    }
}
