//! Minecraft instances (ATLauncher / Prism): version + loader info and the files in mods/,
//! resourcepacks/ and shaderpacks/. ATLauncher tracks mods in instance.json, so we keep that in sync.

#[cfg(not(windows))]
use crate::model::home;
use serde::Serialize;
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize)]
pub struct Instance {
    /// Same id as the library entry, e.g. `atlauncher:Cobblecozy`.
    pub id: String,
    pub name: String,
    pub launcher: String,
    #[serde(skip)]
    pub root: PathBuf,
    #[serde(skip)]
    pub game_dir: PathBuf,
    pub mc_version: String,
    /// neoforge, forge, fabric, quilt or vanilla.
    pub loader: String,
    pub loader_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Mod,
    ResourcePack,
    Shader,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "mod" => Ok(Kind::Mod),
            "resourcepack" => Ok(Kind::ResourcePack),
            "shader" => Ok(Kind::Shader),
            _ => Err(format!("unknown content type {s}")),
        }
    }
    pub fn folder(self) -> &'static str {
        match self {
            Kind::Mod => "mods",
            Kind::ResourcePack => "resourcepacks",
            Kind::Shader => "shaderpacks",
        }
    }
    /// ATLauncher's instance.json "type" value.
    fn atl_type(self) -> &'static str {
        match self {
            Kind::Mod => "mods",
            Kind::ResourcePack => "resourcepack",
            Kind::Shader => "shaderpack",
        }
    }
    pub fn exts(self) -> &'static [&'static str] {
        match self {
            Kind::Mod => &["jar"],
            Kind::ResourcePack | Kind::Shader => &["zip"],
        }
    }
}

#[cfg(not(windows))]
fn atl_dirs() -> Vec<PathBuf> {
    let h = home();
    vec![h.join(".var/app/com.atlauncher.ATLauncher/data/instances"), h.join(".local/share/atlauncher/instances")]
}

#[cfg(not(windows))]
fn prism_dirs() -> Vec<PathBuf> {
    let h = home();
    vec![
        h.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
        h.join(".local/share/PrismLauncher/instances"),
    ]
}

/// Windows: ATLauncher has no fixed folder (it runs from wherever it was unpacked), so it isn't looked for.
#[cfg(windows)]
fn atl_dirs() -> Vec<PathBuf> {
    vec![]
}

/// Windows: Prism Launcher's default data folder, %APPDATA%\PrismLauncher.
#[cfg(windows)]
fn prism_dirs() -> Vec<PathBuf> {
    dirs::data_dir().map(|d| vec![d.join("PrismLauncher").join("instances")]).unwrap_or_default()
}

pub fn all() -> Vec<Instance> {
    let mut out = Vec::new();
    for dir in atl_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let root = e.path();
            let Ok(txt) = fs::read_to_string(root.join("instance.json")) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
            let folder = e.file_name().to_string_lossy().into_owned();
            let l = &v["launcher"];
            let lv = &l["loaderVersion"];
            out.push(Instance {
                id: format!("atlauncher:{folder}"),
                name: l["name"].as_str().unwrap_or(&folder).to_string(),
                launcher: "ATLauncher".into(),
                game_dir: root.clone(),
                root,
                mc_version: v["id"].as_str().or(l["version"].as_str()).unwrap_or("").to_string(),
                loader: lv["type"].as_str().map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "vanilla".into()),
                loader_version: lv["version"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    for dir in prism_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let root = e.path();
            let Ok(cfg) = fs::read_to_string(root.join("instance.cfg")) else { continue };
            let folder = e.file_name().to_string_lossy().into_owned();
            let name = cfg.lines().find_map(|l| l.strip_prefix("name=")).unwrap_or(&folder).to_string();
            let (mut mc, mut loader, mut lver) = (String::new(), "vanilla".to_string(), String::new());
            if let Some(pack) = fs::read_to_string(root.join("mmc-pack.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                for c in pack["components"].as_array().into_iter().flatten() {
                    let ver = c["version"].as_str().unwrap_or("").to_string();
                    match c["uid"].as_str().unwrap_or("") {
                        "net.minecraft" => mc = ver,
                        "net.fabricmc.fabric-loader" => (loader, lver) = ("fabric".into(), ver),
                        "org.quiltmc.quilt-loader" => (loader, lver) = ("quilt".into(), ver),
                        "net.neoforged" => (loader, lver) = ("neoforge".into(), ver),
                        "net.minecraftforge" => (loader, lver) = ("forge".into(), ver),
                        _ => {}
                    }
                }
            }
            let game_dir = [root.join(".minecraft"), root.join("minecraft")].into_iter().find(|p| p.is_dir()).unwrap_or(root.join(".minecraft"));
            out.push(Instance {
                id: format!("prism:{folder}"),
                name,
                launcher: "Prism Launcher".into(),
                root,
                game_dir,
                mc_version: mc,
                loader,
                loader_version: lver,
            });
        }
    }
    out
}

pub fn find(id: &str) -> Result<Instance, String> {
    all().into_iter().find(|i| i.id == id).ok_or_else(|| format!("Instance {id} not found"))
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalFile {
    pub file: String,
    pub enabled: bool,
    pub size: u64,
    pub sha1: String,
    /// Display name from ATLauncher's instance.json when available.
    pub name: Option<String>,
}

impl Instance {
    fn is_atl(&self) -> bool {
        self.id.starts_with("atlauncher:")
    }

    pub fn dir(&self, kind: Kind) -> PathBuf {
        self.game_dir.join(kind.folder())
    }

    /// ATLauncher keeps disabled files in disabledmods/; Prism renames them to *.disabled.
    fn disabled_dir(&self, kind: Kind) -> Option<PathBuf> {
        self.is_atl().then(|| self.game_dir.join("disabledmods")).filter(|_| kind == Kind::Mod)
    }

    pub fn files(&self, kind: Kind) -> Vec<LocalFile> {
        let names = self.atl_names();
        let mut out = Vec::new();
        let mut scan = |dir: &Path, forced_disabled: bool| {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let path = e.path();
                if !path.is_file() {
                    continue;
                }
                let file = e.file_name().to_string_lossy().into_owned();
                let (base, disabled) = match file.strip_suffix(".disabled") {
                    Some(b) => (b.to_string(), true),
                    None => (file.clone(), forced_disabled),
                };
                if !kind.exts().iter().any(|x| base.to_ascii_lowercase().ends_with(&format!(".{x}"))) {
                    continue;
                }
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(LocalFile { name: names.get(&base).cloned(), file, enabled: !disabled, size, sha1: sha1_cached(&path) });
            }
        };
        scan(&self.dir(kind), false);
        if let Some(d) = self.disabled_dir(kind) {
            scan(&d, true);
        }
        out
    }

    fn file_path(&self, kind: Kind, file: &str) -> Result<PathBuf, String> {
        if file.contains('/') || file.starts_with('.') {
            return Err("bad file name".into());
        }
        let candidates = [Some(self.dir(kind).join(file)), self.disabled_dir(kind).map(|d| d.join(file))];
        candidates.into_iter().flatten().find(|p| p.is_file()).ok_or_else(|| format!("{file} not found"))
    }

    pub fn set_enabled(&self, kind: Kind, file: &str, enabled: bool) -> Result<(), String> {
        let from = self.file_path(kind, file)?;
        let base = file.strip_suffix(".disabled").unwrap_or(file);
        let to = match (self.disabled_dir(kind), enabled) {
            (Some(_), true) => self.dir(kind).join(base),
            (Some(d), false) => d.join(base),
            (None, true) => self.dir(kind).join(base),
            (None, false) => self.dir(kind).join(format!("{base}.disabled")),
        };
        if from == to {
            return Ok(());
        }
        if let Some(p) = to.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        fs::rename(&from, &to).map_err(|e| e.to_string())?;
        self.atl_edit(|mods| {
            for m in mods.iter_mut() {
                if m["file"].as_str() == Some(base) {
                    m["disabled"] = Value::Bool(!enabled);
                }
            }
        });
        Ok(())
    }

    /// Moves the file to the desktop trash (recoverable) and drops it from ATLauncher's list.
    pub fn remove(&self, kind: Kind, file: &str) -> Result<(), String> {
        let path = self.file_path(kind, file)?;
        trash(&path)?;
        let base = file.strip_suffix(".disabled").unwrap_or(file).to_string();
        self.atl_edit(|mods| mods.retain(|m| m["file"].as_str() != Some(base.as_str())));
        Ok(())
    }

    /// Record a newly installed file in ATLauncher's instance.json so its UI knows about it.
    pub fn register(&self, kind: Kind, file: &str, name: &str, version: &str, description: &str, source: Option<(&str, &str)>) {
        let file = file.to_string();
        let mut entry = serde_json::json!({
            "name": name,
            "version": version,
            "optional": true,
            "file": file,
            "type": kind.atl_type(),
            "description": description,
            "disabled": false,
            "userAdded": true,
            "wasSelected": true,
            "skipped": false,
        });
        if let Some(("curseforge", id)) = source {
            if let Ok(n) = id.parse::<u64>() {
                entry["curseForgeProjectId"] = Value::from(n);
            }
        }
        self.atl_edit(|mods| {
            mods.retain(|m| m["file"].as_str() != Some(file.as_str()));
            mods.push(entry.clone());
        });
    }

    fn atl_names(&self) -> HashMap<String, String> {
        if !self.is_atl() {
            return HashMap::new();
        }
        let Some(v) = fs::read_to_string(self.root.join("instance.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else {
            return HashMap::new();
        };
        v["launcher"]["mods"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| Some((m["file"].as_str()?.to_string(), m["name"].as_str()?.to_string())))
            .collect()
    }

    fn atl_edit(&self, f: impl FnOnce(&mut Vec<Value>)) {
        if !self.is_atl() {
            return;
        }
        let p = self.root.join("instance.json");
        let Some(mut v) = fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return };
        let backup = self.root.join("instance.json.lodestone-backup");
        if !backup.exists() {
            let _ = fs::copy(&p, &backup);
        }
        if !v["launcher"]["mods"].is_array() {
            v["launcher"]["mods"] = Value::Array(vec![]);
        }
        if let Some(mods) = v["launcher"]["mods"].as_array_mut() {
            f(mods);
        }
        if let Ok(txt) = serde_json::to_string_pretty(&v) {
            let tmp = p.with_extension("json.lodestone-tmp");
            if fs::write(&tmp, txt).is_ok() {
                let _ = fs::rename(tmp, p);
            }
        }
    }
}

fn trash(path: &Path) -> Result<(), String> {
    let ok = crate::os::trash(path);
    if ok {
        Ok(())
    } else {
        fs::remove_file(path).map_err(|e| e.to_string())
    }
}

pub fn sha1_file(path: &Path) -> String {
    let Ok(mut f) = fs::File::open(path) else { return String::new() };
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match f.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => h.update(&buf[..n]),
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Hashing 150+ jars on every view is slow, so remember results by (path, size, mtime).
fn sha1_cached(path: &Path) -> String {
    static CACHE: Mutex<Option<HashMap<(PathBuf, u64, u64), String>>> = Mutex::new(None);
    let meta = fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let key = (path.to_path_buf(), size, mtime);
    if let Some(h) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return h.clone();
    }
    let h = sha1_file(path);
    CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, h.clone());
    h
}
