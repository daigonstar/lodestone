//! Windows: Lodestone runs programs itself, so there is nothing to wrap, no sandbox to reach out of
//! and no host process table to read. This module has the same names as the Linux one (host.rs) for
//! the code both share: starting a program, finding one on the PATH, the list of running programs.

use std::path::PathBuf;
use std::process::Command;

pub struct Proc {
    pub comm: String,
    /// The program's full path stands in for its command line: game detection looks for programs
    /// running from the game's folder.
    pub cmdline: Vec<u8>,
}

/// Programs start as they are.
pub fn wrap(cmd: Command) -> Command {
    cmd
}

/// Every running program except Lodestone itself.
pub fn procs() -> Vec<Proc> {
    crate::os::windows_procs().into_iter().map(|(name, path)| Proc { comm: name, cmdline: path.into_bytes() }).collect()
}

/// Find a program on the PATH, trying the extensions Windows runs (.exe, .cmd, .bat, .com).
pub fn which(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let has_ext = std::path::Path::new(bin).extension().is_some();
    std::env::split_paths(&paths).find_map(|dir| {
        if has_ext {
            let p = dir.join(bin);
            return p.is_file().then_some(p);
        }
        ["exe", "cmd", "bat", "com"].iter().map(|e| dir.join(format!("{bin}.{e}"))).find(|p| p.is_file())
    })
}
