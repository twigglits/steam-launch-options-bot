//! Steam installation discovery: libraries, installed games, users.
//!
//! A game counts as installed only if its appmanifest exists in a currently
//! mounted library AND its steamapps/common/<installdir> folder exists on disk.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::vdf;

/// Compat tools and runtimes ship as "games" and must never be configured.
/// Matched case-insensitively against the start of the name, which is what the
/// Python regex did without needing a regex engine here.
const TOOL_NAME_PREFIXES: [&str; 3] = ["proton", "steam linux runtime", "steamworks common"];

/// Searched in order; the first with a steamapps directory wins. Relative to
/// the user's home.
pub const STEAM_ROOT_CANDIDATES: [&str; 4] = [
    ".local/share/Steam",
    ".steam/steam",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    "snap/steam/common/.local/share/Steam",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Proton,
    Native,
    Unknown,
}

impl Runtime {
    pub fn as_str(&self) -> &'static str {
        match self {
            Runtime::Proton => "proton",
            Runtime::Native => "native",
            Runtime::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub appid: String,
    pub name: String,
    /// Absolute path that exists on disk.
    pub installdir: PathBuf,
    /// The Steam library root containing it.
    pub library: PathBuf,
    pub runtime: Runtime,
}

/// The first Steam root that contains a steamapps directory.
pub fn find_steam_root() -> Option<PathBuf> {
    let home = crate::home();
    for candidate in STEAM_ROOT_CANDIDATES {
        let path = home.join(candidate);
        if path.join("steamapps").is_dir() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }
    None
}

fn load_vdf(path: &Path) -> Option<vdf::Block> {
    let bytes = std::fs::read(path).ok()?;
    vdf::loads(&bytes).ok()
}

/// Bytes from a VDF value into a path, without going through String. A library
/// on a disk whose mount point is not valid UTF-8 still has to be found.
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// All library roots from libraryfolders.vdf that are currently mounted.
pub fn library_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.to_path_buf()];
    let listing = root.join("steamapps").join("libraryfolders.vdf");
    if !listing.is_file() {
        return paths;
    }
    let Some(data) = load_vdf(&listing) else {
        return paths;
    };
    let Some(folders) = data.get_block(b"libraryfolders") else {
        return paths;
    };
    for (_, value) in folders.iter() {
        let Some(entry) = value.as_block() else {
            continue;
        };
        let Some(raw) = entry.get_str(b"path") else {
            continue;
        };
        let path = path_from_bytes(raw);
        if path != root && path.join("steamapps").is_dir() {
            paths.push(path);
        }
    }
    paths
}

/// Per-appid compat tool names from config.vdf CompatToolMapping.
pub fn compat_mapping(root: &Path) -> BTreeMap<String, String> {
    let empty = BTreeMap::new();
    let config = root.join("config").join("config.vdf");
    if !config.is_file() {
        return empty;
    }
    let Some(data) = load_vdf(&config) else {
        return empty;
    };
    let mut node = &data;
    for key in [
        b"InstallConfigStore".as_slice(),
        b"Software",
        b"Valve",
        b"Steam",
        b"CompatToolMapping",
    ] {
        match node.get_block(key) {
            Some(child) => node = child,
            None => return empty,
        }
    }
    node.iter()
        .filter_map(|(appid, value)| {
            let entry = value.as_block()?;
            let name = entry.get_str(b"name").unwrap_or(b"");
            Some((text(appid), text(name)))
        })
        .collect()
}

fn resolve_runtime(appid: &str, library: &Path, mapping: &BTreeMap<String, String>) -> Runtime {
    // The global "0" mapping only affects titles that *need* compat, which we
    // cannot know offline, so only per-app signals are trusted.
    if mapping.get(appid).is_some_and(|name| !name.is_empty()) {
        return Runtime::Proton;
    }
    if library
        .join("steamapps")
        .join("compatdata")
        .join(appid)
        .is_dir()
    {
        return Runtime::Proton;
    }
    Runtime::Native
}

fn is_tool(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    TOOL_NAME_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
}

fn manifest_paths(steamapps: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(steamapps) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("appmanifest_") && name.ends_with(".acf"))
        })
        .collect();
    // Sorted because change ordering is observable in the NDJSON stream, and
    // read_dir order is whatever the filesystem feels like.
    paths.sort();
    paths
}

/// Games whose manifest and install folder both exist, tools excluded.
pub fn installed_games(root: &Path) -> Vec<Game> {
    let mapping: BTreeMap<String, String> = compat_mapping(root)
        .into_iter()
        .filter(|(appid, _)| appid != "0")
        .collect();

    let mut games = Vec::new();
    for library in library_paths(root) {
        let steamapps = library.join("steamapps");
        for manifest in manifest_paths(&steamapps) {
            let Some(data) = load_vdf(&manifest) else {
                continue;
            };
            let Some(state) = data.get_block(b"AppState") else {
                continue;
            };
            let appid = text(state.get_str(b"appid").unwrap_or(b""));
            let name = text(state.get_str(b"name").unwrap_or(b""));
            let installdir = state.get_str(b"installdir").unwrap_or(b"");

            if appid.is_empty() || installdir.is_empty() || is_tool(&name) {
                continue;
            }
            let path = steamapps.join("common").join(path_from_bytes(installdir));
            if !path.is_dir() {
                continue;
            }
            let runtime = resolve_runtime(&appid, &library, &mapping);
            games.push(Game {
                appid,
                name,
                installdir: path,
                library: library.clone(),
                runtime,
            });
        }
    }
    games
}

/// (accountid, localconfig.vdf path) for every Steam user on this machine.
pub fn user_localconfigs(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("userdata")) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let config = dir.join("config").join("localconfig.vdf");
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) && config.is_file() {
            out.push((name.to_string(), config));
        }
    }
    out
}

/// True if the Steam client owning this root is currently running.
///
/// Steam writes ~/.steam/steam.pid and symlinks ~/.steam/steam to its root;
/// the global pid file is only trusted when that symlink resolves to `root`,
/// so checks against fixture roots stay deterministic.
pub fn is_steam_running(root: &Path) -> bool {
    let mut candidates = vec![root.join("steam.pid")];
    let home = crate::home();
    if let (Ok(link), Ok(target)) = (
        home.join(".steam/steam").canonicalize(),
        root.canonicalize(),
    ) {
        if link == target {
            candidates.push(home.join(".steam/steam.pid"));
        }
    }
    for pid_file in candidates {
        let Ok(text) = std::fs::read_to_string(&pid_file) else {
            continue;
        };
        let Ok(pid) = text.trim().parse::<u32>() else {
            continue;
        };
        let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
            continue;
        };
        if comm.trim() == "steam" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_matched_case_insensitively() {
        assert!(is_tool("Proton 9.0"));
        assert!(is_tool("proton experimental"));
        assert!(is_tool("Steam Linux Runtime 3.0 (sniper)"));
        assert!(is_tool("Steamworks Common Redistributables"));
        assert!(!is_tool("The Witcher 3"));
        assert!(
            !is_tool("Half-Life: Proton Edition"),
            "the match is anchored"
        );
        // Faithful to the Python regex, which anchors at the start but does
        // not require a word boundary: a game actually called "Protonophore"
        // would be skipped. Left as-is rather than quietly tightened - it is a
        // behaviour change, and no such game exists to be broken by it.
        assert!(is_tool("Protonophore"));
    }

    #[test]
    fn runtime_strings_match_the_wire_contract() {
        assert_eq!(Runtime::Proton.as_str(), "proton");
        assert_eq!(Runtime::Native.as_str(), "native");
        assert_eq!(Runtime::Unknown.as_str(), "unknown");
    }
}
