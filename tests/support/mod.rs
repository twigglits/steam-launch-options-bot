//! Fixture builders shared by the integration tests.
//!
//! Ported from the Python tests' `make_steam_root` / `make_manifest`, which
//! several test modules imported from `tests/test_steam.py`.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use steamtrain::steam::{Game, Runtime};
use steamtrain::sysinfo::SystemProfile;

/// The Python tests' `fake_profile`: a fixed profile with one dial, so a test
/// never depends on the machine it runs on.
pub fn fake_profile(vendor: &str) -> SystemProfile {
    SystemProfile {
        distro: "Arch Linux".to_string(),
        kernel: "6.9.0".to_string(),
        desktop: "KDE".to_string(),
        session: "wayland".to_string(),
        gpu_vendor: vendor.to_string(),
        gpu_name: String::new(),
        gpu_driver: String::new(),
        cpu_threads: 8,
        ram_gb: 16,
        has_gamemode: false,
        has_mangohud: false,
        has_gamescope: false,
    }
}

pub fn fake_game(appid: &str, runtime: Runtime) -> Game {
    Game {
        appid: appid.to_string(),
        name: "Fixture Game".to_string(),
        installdir: PathBuf::from("/tmp/FixtureGame"),
        library: PathBuf::from("/tmp"),
        runtime,
    }
}

/// A Steam root with a steamapps directory and an empty libraryfolders.vdf.
pub fn make_steam_root(base: &Path) -> PathBuf {
    let root = base.join("Steam");
    fs::create_dir_all(root.join("steamapps/common")).unwrap();
    fs::write(
        root.join("steamapps/libraryfolders.vdf"),
        "\"libraryfolders\"\n{\n}\n",
    )
    .unwrap();
    root
}

/// An appmanifest plus the install folder it points at, so the game counts as
/// installed: manifest present AND steamapps/common/<installdir> on disk.
pub fn make_manifest(root: &Path, appid: &str, name: &str, installdir: &str) {
    let steamapps = root.join("steamapps");
    fs::create_dir_all(steamapps.join("common").join(installdir)).unwrap();
    fs::write(
        steamapps.join(format!("appmanifest_{appid}.acf")),
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{installdir}\"\n}}\n"
        ),
    )
    .unwrap();
}

/// A localconfig.vdf for one Steam account, with no apps block yet.
pub fn make_user(root: &Path, account: &str) -> PathBuf {
    let config = root.join("userdata").join(account).join("config");
    fs::create_dir_all(&config).unwrap();
    let path = config.join("localconfig.vdf");
    fs::write(&path, "\"UserLocalConfigStore\"\n{\n}\n").unwrap();
    path
}

/// A config.vdf whose CompatToolMapping names a tool for each given appid.
pub fn make_compat_mapping(root: &Path, entries: &[(&str, &str)]) {
    fs::create_dir_all(root.join("config")).unwrap();
    let mut body = String::new();
    for (appid, tool) in entries {
        body.push_str(&format!(
            "\t\t\t\t\t\"{appid}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"{tool}\"\n\t\t\t\t\t}}\n"
        ));
    }
    fs::write(
        root.join("config/config.vdf"),
        format!(
            "\"InstallConfigStore\"\n{{\n\t\"Software\"\n\t{{\n\t\t\"Valve\"\n\t\t{{\n\t\t\t\"Steam\"\n\t\t\t{{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{{\n{body}\t\t\t\t}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
        ),
    )
    .unwrap();
}

/// The LaunchOptions currently recorded for one appid in a localconfig.vdf.
pub fn current_options(localconfig: &Path, appid: &str) -> String {
    let data = steamtrain::vdf::loads(&fs::read(localconfig).unwrap()).unwrap();
    let mut node = &data;
    for name in ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"] {
        match node.get_block(name.as_bytes()) {
            Some(child) => node = child,
            None => return String::new(),
        }
    }
    node.get_block(appid.as_bytes())
        .and_then(|block| block.get_str(b"LaunchOptions"))
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .unwrap_or_default()
}

/// Overwrite one appid's LaunchOptions, standing in for a human editing it in
/// the Steam client.
pub fn set_options(localconfig: &Path, appid: &str, value: &str) {
    use steamtrain::vdf::Value;

    let mut data = steamtrain::vdf::loads(&fs::read(localconfig).unwrap()).unwrap();
    {
        let apps = data
            .child_ci(b"UserLocalConfigStore")
            .child_ci(b"Software")
            .child_ci(b"Valve")
            .child_ci(b"Steam")
            .child_ci(b"apps");
        apps.child_ci(appid.as_bytes()).insert(
            b"LaunchOptions".to_vec(),
            Value::Str(value.as_bytes().to_vec()),
        );
    }
    fs::write(localconfig, steamtrain::vdf::dumps(&data)).unwrap();
}
