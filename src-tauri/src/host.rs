//! Running as a Flatpak. The sandbox can't see the host's programs (umu-run, Steam, Wine) or processes,
//! so when Lodestone is a Flatpak, programs it starts run on the host through `flatpak-spawn --host`,
//! and the process list comes from the host's /proc. Outside a Flatpak everything here is a pass-through.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Names the variables a self-run helper (`--gog-setup`, `--bnet-play`) must pass on to what it runs
/// on the host, since the host side starts from the desktop session's environment.
pub const FWD_ENV: &str = "LODESTONE_FWD_ENV";

pub fn in_flatpak() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| std::path::Path::new("/.flatpak-info").exists())
}

/// Inside the Flatpak, point the XDG folders back at the usual ones, so the Flatpak and a native
/// install share settings, library data and store sign-ins.
pub fn use_host_dirs() {
    if !in_flatpak() {
        return;
    }
    let home = crate::model::home();
    for (k, d) in [("XDG_DATA_HOME", ".local/share"), ("XDG_CONFIG_HOME", ".config"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")] {
        std::env::set_var(k, home.join(d));
    }
}

/// The command for running a program on the host (unchanged outside a Flatpak). Build the command,
/// wrap it, then set stdio and process options on what comes back.
pub fn wrap(cmd: Command) -> Command {
    if !in_flatpak() {
        return cmd;
    }
    let mut out = Command::new("flatpak-spawn");
    out.arg("--host");
    if let Some(d) = cmd.get_current_dir() {
        out.arg(format!("--directory={}", d.display()));
    }
    let set: HashSet<&OsStr> = cmd.get_envs().map(|(k, _)| k).collect();
    if let Ok(keys) = std::env::var(FWD_ENV) {
        for k in keys.split(',').filter(|k| !k.is_empty() && !set.contains(OsStr::new(k))) {
            if let Ok(v) = std::env::var(k) {
                out.arg(format!("--env={k}={v}"));
            }
        }
    }
    // Removed variables need nothing: the host side never had Lodestone's own environment.
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            out.arg(format!("--env={}={}", k.to_string_lossy(), v.to_string_lossy()));
        }
    }
    out.arg(cmd.get_program()).args(cmd.get_args());
    out
}

/// How a helper launch runs Lodestone itself: this binary, or the Flatpak through `flatpak run`.
pub fn self_command() -> Option<(String, Vec<String>)> {
    if in_flatpak() {
        let id = std::env::var("FLATPAK_ID").ok()?;
        return Some(("flatpak".into(), vec!["run".into(), "--command=lodestone".into(), id]));
    }
    let me = std::env::current_exe().ok()?;
    Some((me.to_string_lossy().into_owned(), vec![]))
}

/// Find a program on the PATH (the host's, from a Flatpak; checked at most once a minute).
pub fn which(bin: &str) -> Option<PathBuf> {
    if !in_flatpak() {
        return std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(bin)).find(|p| p.is_file()));
    }
    static CACHE: Mutex<Option<HashMap<String, (Instant, Option<PathBuf>)>>> = Mutex::new(None);
    if let Some((at, p)) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(bin) {
        if at.elapsed() < Duration::from_secs(60) {
            return p.clone();
        }
    }
    let out = Command::new("flatpak-spawn").args(["--host", "sh", "-c", "command -v \"$1\"", "sh", bin]).stdin(Stdio::null()).output().ok();
    let p = out
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| s.starts_with('/'))
        .map(PathBuf::from);
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(bin.into(), (Instant::now(), p.clone()));
    p
}

pub struct Proc {
    pub comm: String,
    /// Raw /proc/<pid>/cmdline: arguments, each ending in NUL.
    pub cmdline: Vec<u8>,
}

/// Every process (the host's, from a Flatpak) except Lodestone itself.
pub fn procs() -> Vec<Proc> {
    #[cfg(windows)]
    {
        // The program's full path stands in for its command line: game detection looks for programs
        // running from the game's folder.
        return crate::os::windows_procs().into_iter().map(|(name, path)| Proc { comm: name, cmdline: path.into_bytes() }).collect();
    }
    #[allow(unreachable_code)]
    if in_flatpak() {
        return host_procs();
    }
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            if pid == me {
                return None;
            }
            let comm = std::fs::read_to_string(e.path().join("comm")).ok()?.trim_end().to_string();
            let cmdline = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            Some(Proc { comm, cmdline })
        })
        .collect()
}

/// One host command for the whole table: `head` prints every file with a `==> name <==` header.
fn host_procs() -> Vec<Proc> {
    let out = Command::new("flatpak-spawn")
        .args(["--host", "sh", "-c", "cd /proc && exec head -c 32768 [0-9]*/comm [0-9]*/cmdline 2>/dev/null"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else { return vec![] };
    let mut by_pid: HashMap<u32, Proc> = HashMap::new();
    for (name, data) in split_head(&out.stdout) {
        let Some((pid, file)) = name.split_once('/') else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        let p = by_pid.entry(pid).or_insert_with(|| Proc { comm: String::new(), cmdline: vec![] });
        match file {
            "comm" => p.comm = String::from_utf8_lossy(data).trim_end().to_string(),
            "cmdline" => p.cmdline = data.to_vec(),
            _ => {}
        }
    }
    // The shell that read the table.
    by_pid.into_values().filter(|p| !p.comm.is_empty() && p.comm != "head").collect()
}

/// Split `head`'s multi-file output into (file name, contents).
fn split_head(b: &[u8]) -> Vec<(String, &[u8])> {
    const OPEN: &[u8] = b"==> ";
    const CLOSE: &[u8] = b" <==\n";
    let find = |from: usize, pat: &[u8]| b[from..].windows(pat.len()).position(|w| w == pat).map(|i| i + from);
    // Header starts: at the very beginning or right after a newline.
    let mut heads = Vec::new();
    let mut i = 0;
    while let Some(at) = find(i, OPEN) {
        if at == 0 || b[at - 1] == b'\n' {
            if let Some(end) = find(at, CLOSE) {
                if !b[at..end].contains(&b'\n') {
                    heads.push((at, end + CLOSE.len(), String::from_utf8_lossy(&b[at + OPEN.len()..end]).into_owned()));
                    i = end + CLOSE.len();
                    continue;
                }
            }
        }
        i = at + 1;
    }
    let mut out = Vec::new();
    for (n, (_, body, name)) in heads.iter().enumerate() {
        // head puts a blank line before every header after the first.
        let end = heads.get(n + 1).map(|h| h.0.saturating_sub(1)).unwrap_or(b.len()).max(*body);
        out.push((name.clone(), &b[*body..end]));
    }
    out
}

/// Each running wineserver's WINEPREFIX and the socket inodes it holds open.
pub fn wineservers() -> Vec<(String, HashSet<String>)> {
    if in_flatpak() {
        let script = r#"for p in $(pgrep -x wineserver); do printf '\036'; tr '\0' '\n' < /proc/$p/environ | grep '^WINEPREFIX='; ls -l /proc/$p/fd 2>/dev/null | grep -o 'socket:\[[0-9]*\]'; done"#;
        let Ok(out) = Command::new("flatpak-spawn").args(["--host", "sh", "-c", script]).stdin(Stdio::null()).stderr(Stdio::null()).output() else { return vec![] };
        return String::from_utf8_lossy(&out.stdout)
            .split('\u{1e}')
            .filter_map(|rec| {
                let pfx = rec.lines().find_map(|l| l.strip_prefix("WINEPREFIX="))?.to_string();
                let inodes = rec.lines().filter_map(|l| l.strip_prefix("socket:[")?.strip_suffix(']').map(String::from)).collect();
                Some((pfx, inodes))
            })
            .collect();
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let p = e.path();
        if std::fs::read_to_string(p.join("comm")).map(|c| c.trim() != "wineserver").unwrap_or(true) {
            continue;
        }
        let env = std::fs::read(p.join("environ")).unwrap_or_default();
        let Some(pfx) = env.split(|b| *b == 0).find_map(|kv| String::from_utf8_lossy(kv).strip_prefix("WINEPREFIX=").map(String::from)) else { continue };
        let inodes = std::fs::read_dir(p.join("fd"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|fd| std::fs::read_link(fd.path()).ok()?.to_string_lossy().strip_prefix("socket:[")?.strip_suffix(']').map(String::from))
            .collect();
        out.push((pfx, inodes));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_head_output() {
        let raw = b"==> 12/comm <==\nsteam\n\n==> 12/cmdline <==\n/usr/bin/steam\0-silent\0\n==> 7/cmdline <==\na b\0";
        let parts = super::split_head(raw);
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names, ["12/comm", "12/cmdline", "7/cmdline"]);
        assert_eq!(parts[0].1, b"steam\n");
        assert_eq!(parts[1].1, b"/usr/bin/steam\0-silent\0");
        assert_eq!(parts[2].1, b"a b\0");
    }

    #[test]
    fn passes_through_outside_flatpak() {
        let mut c = std::process::Command::new("echo");
        c.arg("hi").env("A", "1");
        let w = super::wrap(c);
        assert_eq!(w.get_program(), "echo");
    }
}
