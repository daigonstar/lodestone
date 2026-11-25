//! The few things that differ between Linux and Windows below the app itself: keeping files private,
//! marking programs executable, starting games detached from Lodestone, free disk space, and the list
//! of running programs. Everything else is shared.
//!
//! Privacy on Windows: Lodestone's data lives in %APPDATA%, which Windows already makes private to your
//! user account (only you and administrators can read it), so there's nothing extra to set there. On
//! Linux, files holding sign-in data are 600 and their folders 700, from the moment they're created.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

/// Write a file only you can read (it holds keys or sign-in data).
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut o = fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    // Files made before this rule existed.
    set_private(path, false);
    f.write_all(bytes)
}

/// Make a file (or, with `dir`, a folder) readable only by you. On Windows %APPDATA% already is.
pub fn set_private(path: &Path, dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(if dir { 0o700 } else { 0o600 }));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, dir);
}

/// Mark a downloaded program executable (Windows decides by the file's extension).
pub fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Start a game detached from Lodestone: it keeps running if Lodestone closes, and Lodestone's
/// terminal (if any) can't send it signals.
pub fn detach(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A new process group, and no console window of its own.
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}

/// Free space on the drive holding `p`, in bytes (0 if unknown).
pub fn free_space(p: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return 0 };
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return 0;
        }
        s.f_bavail as u64 * s.f_frsize as u64
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        if ok == 0 {
            0
        } else {
            free
        }
    }
}

/// Disk space a file really takes. On Linux that's the blocks written (Steam reserves full-size files
/// up front, which would otherwise look finished straight away); on Windows, the file's size.
pub fn file_disk_use(m: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        m.blocks() * 512
    }
    #[cfg(not(unix))]
    {
        m.len()
    }
}

/// Windows: the running programs, each with its full path (what game detection matches against:
/// is something running from this game's folder?).
#[cfg(windows)]
pub fn windows_procs() -> Vec<(String, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    let me = std::process::id();
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut e) != 0;
        while more {
            if e.th32ProcessID != me && e.th32ProcessID != 0 {
                let name_len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..name_len]);
                let mut path = String::new();
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                if !h.is_null() {
                    let mut buf = [0u16; 1024];
                    let mut len = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                        path = String::from_utf16_lossy(&buf[..len as usize]);
                    }
                    CloseHandle(h);
                }
                out.push((name, path));
            }
            more = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}
