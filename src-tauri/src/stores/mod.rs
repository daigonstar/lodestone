pub mod bnet_agent;
pub mod bnet_catalog;
pub mod custom;
pub mod ea_api;
pub mod emulation;
pub mod epic;
pub mod gog;
pub mod heroic;
pub mod lutris;
pub mod minecraft;
pub mod steam;
pub mod ubisoft_api;
pub mod winapps;

use crate::model::Game;

/// Scan every source. Each scanner is independent and silently returns nothing if its launcher isn't present.
pub fn scan_all() -> Vec<Game> {
    // Lodestone's own Epic/GOG first so they win over Heroic's copy of the same game.
    let scanners: [fn() -> Vec<Game>; 9] = [epic::scan, gog::scan, steam::scan, heroic::scan, lutris::scan, minecraft::scan, custom::scan, winapps::scan, emulation::scan];
    let handles: Vec<_> = scanners.into_iter().map(std::thread::spawn).collect();
    let mut all: Vec<Game> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for g in handles.into_iter().flat_map(|h| h.join().unwrap_or_default()) {
        match index.get(&g.id) {
            // Same game from two sources: keep the installed one, otherwise the first (Lodestone's).
            Some(&i) => {
                if g.installed && !all[i].installed {
                    all[i] = g;
                }
            }
            None => {
                index.insert(g.id.clone(), all.len());
                all.push(g);
            }
        }
    }
    all.sort_by_key(|g| g.title.to_lowercase());
    all
}

#[cfg(test)]
mod tests {
    /// `cargo test print_library -- --ignored --nocapture` to see what gets detected on this machine.
    #[test]
    #[ignore]
    fn live_updates() {
        for g in super::scan_all().iter().filter(|g| g.update.is_some()) {
            println!("{:<40} {:<8} {:?} via {:?} size {:?}", g.title, g.store, g.update, g.update_via, g.update_size);
        }
    }

    /// Writes the scanned library as JSON to $LODESTONE_DUMP (for checking UI logic against real data).
    #[test]
    #[ignore]
    fn dump_library_json() {
        let all = super::scan_all();
        std::fs::write(std::env::var("LODESTONE_DUMP").unwrap(), serde_json::to_string(&all).unwrap()).unwrap();
    }

    #[test]
    #[ignore]
    fn print_library() {
        let all = super::scan_all();
        for g in all.iter().filter(|g| g.installed) {
            println!("{:<10} {:<45} {:<22} cover={} detect={:?}\n           {:?}", g.store, g.title, g.via, g.cover.is_some(), g.detect, g.launch.as_ref().map(|l| (&l.program, &l.args)));
        }
        println!("installed: {}  total incl. not installed: {}", all.iter().filter(|g| g.installed).count(), all.len());
        let steam = all.iter().filter(|g| g.store == "steam").count();
        println!("steam entries: {steam}; FF7 Rebirth: {:?}", all.iter().find(|g| g.id == "steam:2909400").map(|g| (&g.title, g.installed, g.cover.is_some())));
    }
}
