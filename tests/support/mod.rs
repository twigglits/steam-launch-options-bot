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

// --------------------------------------------------------------- CLI harness

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use steamtrain::cli;
use steamtrain::doctor;
use steamtrain::proc::{CommandRunner, Output, RunError};
use steamtrain::sysinfo::SystemProbe;

/// Canned /proc and /etc content that makes `sysinfo::detect` produce the same
/// profile `fake_profile` returns, so a CLI test never depends on the machine
/// running it.
pub struct FakeProbe {
    files: HashMap<String, String>,
    env: HashMap<String, String>,
    programs: Vec<String>,
    command_output: Option<String>,
}

impl FakeProbe {
    pub fn with_vendor(vendor: &str) -> Self {
        let mut files = HashMap::new();
        files.insert(
            "/etc/os-release".to_string(),
            "PRETTY_NAME=\"Arch Linux\"\n".to_string(),
        );
        files.insert(
            "/proc/sys/kernel/osrelease".to_string(),
            "6.9.0\n".to_string(),
        );
        files.insert("/proc/cpuinfo".to_string(), "processor\t: 0\n".repeat(8));
        files.insert(
            "/proc/meminfo".to_string(),
            "MemTotal:       16777216 kB\n".to_string(),
        );
        match vendor {
            "nvidia" => {
                files.insert(
                    "/sys/module/nvidia/version".to_string(),
                    "595.71.05\n".to_string(),
                );
            }
            "amd" => {
                files.insert(
                    "/proc/modules".to_string(),
                    "amdgpu 1 0 - Live\n".to_string(),
                );
            }
            "intel" => {
                files.insert("/proc/modules".to_string(), "i915 1 0 - Live\n".to_string());
            }
            _ => {
                files.insert("/proc/modules".to_string(), "loop 1 0 - Live\n".to_string());
            }
        }
        let mut env = HashMap::new();
        env.insert("XDG_CURRENT_DESKTOP".to_string(), "KDE".to_string());
        env.insert("XDG_SESSION_TYPE".to_string(), "wayland".to_string());
        FakeProbe {
            files,
            env,
            programs: Vec::new(),
            command_output: None,
        }
    }

    pub fn with_program(mut self, name: &str) -> Self {
        self.programs.push(name.to_string());
        self
    }
}

impl SystemProbe for FakeProbe {
    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }
    fn read_text(&self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }
    fn which(&self, name: &str) -> Option<PathBuf> {
        self.programs
            .iter()
            .any(|program| program == name)
            .then(|| PathBuf::from(format!("/usr/bin/{name}")))
    }
    fn path_exists(&self, _path: &Path) -> bool {
        false
    }
    fn run(&self, _argv: &[String], _timeout: Duration) -> Option<String> {
        self.command_output.clone()
    }
}

#[derive(Default)]
pub struct FakeRunner {
    pub stdout: String,
    pub status: Option<i32>,
    pub calls: Mutex<Vec<Vec<String>>>,
}

impl FakeRunner {
    pub fn replying(stdout: &str) -> Self {
        FakeRunner {
            stdout: stdout.to_string(),
            status: Some(0),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        argv: &[String],
        _input: Option<&str>,
        _timeout: Duration,
    ) -> Result<Output, RunError> {
        self.calls.lock().unwrap().push(argv.to_vec());
        Ok(Output {
            status: self.status.or(Some(0)),
            stdout: self.stdout.clone(),
            stderr: String::new(),
        })
    }
}

/// Drives `cli::main` in process, the way the Python tests called `cli.main`
/// under redirect_stdout.
pub struct Cli {
    pub probe: FakeProbe,
    pub runner: FakeRunner,
    pub steam_running: bool,
    pub doctor: doctor::Options,
}

impl Cli {
    pub fn new(vendor: &str) -> Self {
        Cli {
            probe: FakeProbe::with_vendor(vendor),
            runner: FakeRunner::default(),
            steam_running: false,
            // A path that cannot exist, so warn_legacy stays silent and the
            // tests do not depend on whether the machine has the package.
            doctor: doctor::Options {
                home: None,
                path_env: Some("/nonexistent".to_string()),
                packaged_bin: PathBuf::from("/nonexistent/steamtrain"),
                force: false,
            },
        }
    }

    pub fn run(&self, argv: &[&str], stdin: &str) -> Run {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut input = std::io::Cursor::new(stdin.as_bytes().to_vec());
        let running = self.steam_running;
        let is_running = move |_: &Path| running;

        let code = {
            let mut io = cli::Io {
                out: &mut out,
                err: &mut err,
                input: &mut input,
            };
            let deps = cli::Deps {
                probe: &self.probe,
                is_running: &is_running,
                runner: &self.runner,
                doctor: self.doctor.clone(),
            };
            let owned: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
            cli::main(&owned, &mut io, &deps)
        };

        Run {
            code,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }
}

pub struct Run {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Run {
    pub fn records(&self) -> Vec<serde_json::Value> {
        self.out
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|err| panic!("not JSON: {line:?} ({err})"))
            })
            .collect()
    }

    pub fn of_kind(&self, kind: &str) -> Vec<serde_json::Value> {
        self.records()
            .into_iter()
            .filter(|record| record["kind"] == kind)
            .collect()
    }

    /// The terminal record. Absent means the stream was truncated.
    pub fn result(&self) -> serde_json::Value {
        let records = self.records();
        let last = records.last().expect("at least one record");
        assert_eq!(last["kind"], "result", "stream did not end with a result");
        assert_eq!(
            records.iter().filter(|r| r["kind"] == "result").count(),
            1,
            "exactly one result record"
        );
        last.clone()
    }
}
