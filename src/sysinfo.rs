//! System profile detection: OS, desktop environment, GPU, CPU, helper tools.
//!
//! Every impure input is injectable, so the whole module is testable without a
//! desktop session. That matters in production too, not only in tests: the CLI
//! is also run from cron, ssh and a user's own systemd unit, where there is no
//! session to inherit variables from, and the wayland socket in
//! XDG_RUNTIME_DIR is the fallback.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

/// Field order here is the field order of the `profile` record on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemProfile {
    pub distro: String,
    pub kernel: String,
    pub desktop: String,
    /// 'wayland' | 'x11' | 'unknown'
    pub session: String,
    /// 'nvidia' | 'amd' | 'intel' | 'unknown'
    pub gpu_vendor: String,
    pub gpu_name: String,
    pub gpu_driver: String,
    pub cpu_threads: u32,
    pub ram_gb: u64,
    pub has_gamemode: bool,
    pub has_mangohud: bool,
    pub has_gamescope: bool,
}

/// How long nvidia-smi gets before it is killed. Matches the Python's
/// subprocess timeout.
const SMI_TIMEOUT: Duration = Duration::from_secs(10);

pub trait SystemProbe {
    fn env(&self, key: &str) -> Option<String>;
    fn read_text(&self, path: &str) -> Option<String>;
    fn which(&self, name: &str) -> Option<PathBuf>;
    fn path_exists(&self, path: &Path) -> bool;
    /// Stdout of a successful run, or None on any failure. Detection degrades
    /// to a less specific answer rather than propagating an error.
    fn run(&self, argv: &[String], timeout: Duration) -> Option<String>;
}

pub struct RealProbe;

impl SystemProbe for RealProbe {
    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn read_text(&self, path: &str) -> Option<String> {
        // Lossy: everything read here is /proc or /etc metadata used for
        // display and matching, never written back anywhere.
        std::fs::read(path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        crate::proc::which(name, None)
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn run(&self, argv: &[String], timeout: Duration) -> Option<String> {
        crate::proc::run(argv, None, timeout)
            .ok()
            .filter(crate::proc::Output::succeeded)
            .map(|output| output.stdout)
    }
}

pub fn detect(probe: &dyn SystemProbe) -> SystemProfile {
    let cpuinfo = probe.read_text("/proc/cpuinfo").unwrap_or_default();
    let counted = cpuinfo.matches("processor\t").count() as u32;
    let cpu_threads = if counted > 0 {
        counted
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
    };

    let (gpu_vendor, gpu_name, gpu_driver) = detect_gpu(probe);

    SystemProfile {
        distro: detect_distro(probe),
        kernel: detect_kernel(probe),
        desktop: detect_desktop(probe),
        session: detect_session(probe),
        gpu_vendor,
        gpu_name,
        gpu_driver,
        cpu_threads,
        ram_gb: detect_ram_gb(probe),
        has_gamemode: probe.which("gamemoderun").is_some(),
        has_mangohud: probe.which("mangohud").is_some(),
        has_gamescope: probe.which("gamescope").is_some(),
    }
}

fn detect_distro(probe: &dyn SystemProbe) -> String {
    let text = probe.read_text("/etc/os-release").unwrap_or_default();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return value.trim().trim_matches('"').to_string();
        }
    }
    "unknown".to_string()
}

fn detect_kernel(probe: &dyn SystemProbe) -> String {
    // Python called os.uname(). Reading the same value through the injectable
    // reader costs nothing and makes it testable.
    probe
        .read_text("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn detect_desktop(probe: &dyn SystemProbe) -> String {
    match probe.env("XDG_CURRENT_DESKTOP") {
        // "ubuntu:GNOME" -> "GNOME"
        Some(desktop) if !desktop.is_empty() => {
            desktop.rsplit(':').next().unwrap_or(&desktop).to_string()
        }
        _ => "unknown".to_string(),
    }
}

fn detect_session(probe: &dyn SystemProbe) -> String {
    if let Some(session) = probe.env("XDG_SESSION_TYPE") {
        if session == "wayland" || session == "x11" {
            return session;
        }
    }
    let runtime_dir = probe
        .env("XDG_RUNTIME_DIR")
        .or_else(|| current_uid(probe).map(|uid| format!("/run/user/{uid}")));
    if let Some(dir) = runtime_dir {
        if probe.path_exists(&Path::new(&dir).join("wayland-0")) {
            return "wayland".to_string();
        }
    }
    if probe.env("DISPLAY").is_some_and(|value| !value.is_empty()) {
        return "x11".to_string();
    }
    "unknown".to_string()
}

/// The real uid, for the `/run/user/<uid>` fallback when XDG_RUNTIME_DIR is
/// unset. Read from /proc rather than getuid(), which std does not expose and
/// which would otherwise mean taking a libc dependency for one integer.
fn current_uid(probe: &dyn SystemProbe) -> Option<u32> {
    let status = probe.read_text("/proc/self/status")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn detect_ram_gb(probe: &dyn SystemProbe) -> u64 {
    let meminfo = probe.read_text("/proc/meminfo").unwrap_or_default();
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|field| field.parse::<u64>().ok())
                .map(|kb| kb / (1024 * 1024))
                .unwrap_or(0);
        }
    }
    0
}

/// (vendor, name, driver) from kernel modules and sysfs.
fn detect_gpu(probe: &dyn SystemProbe) -> (String, String, String) {
    let nvidia_version = probe
        .read_text("/sys/module/nvidia/version")
        .filter(|text| !text.is_empty());
    if let Some(version) = nvidia_version {
        let mut name = String::new();
        if let Some(smi) = probe.which("nvidia-smi") {
            let argv = vec![
                smi.to_string_lossy().into_owned(),
                "--query-gpu=name".to_string(),
                "--format=csv,noheader".to_string(),
            ];
            if let Some(stdout) = probe.run(&argv, SMI_TIMEOUT) {
                name = stdout.trim().lines().next().unwrap_or("").to_string();
            }
        }
        if name.is_empty() {
            name = "NVIDIA GPU".to_string();
        }
        return ("nvidia".to_string(), name, version.trim().to_string());
    }

    let modules = probe.read_text("/proc/modules").unwrap_or_default();
    let loaded: HashSet<&str> = modules
        .lines()
        .filter_map(|line| line.split(' ').next())
        .collect();
    if loaded.contains("amdgpu") || loaded.contains("radeon") {
        return (
            "amd".to_string(),
            "AMD GPU".to_string(),
            "amdgpu (Mesa)".to_string(),
        );
    }
    if loaded.contains("i915") || loaded.contains("xe") {
        return (
            "intel".to_string(),
            "Intel GPU".to_string(),
            "i915/xe (Mesa)".to_string(),
        );
    }
    ("unknown".to_string(), String::new(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    pub struct FakeProbe {
        env: HashMap<String, String>,
        files: HashMap<String, String>,
        found: Vec<String>,
        exists: Vec<String>,
        command_output: Option<String>,
    }

    impl FakeProbe {
        fn new(env: &[(&str, &str)], files: &[(&str, &str)]) -> Self {
            FakeProbe {
                env: env
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                files: files
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                found: Vec::new(),
                exists: Vec::new(),
                command_output: None,
            }
        }

        fn with_program(mut self, name: &str) -> Self {
            self.found.push(name.to_string());
            self
        }

        fn with_path(mut self, path: &str) -> Self {
            self.exists.push(path.to_string());
            self
        }

        fn with_command_output(mut self, output: &str) -> Self {
            self.command_output = Some(output.to_string());
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
            self.found
                .iter()
                .any(|found| found == name)
                .then(|| PathBuf::from(format!("/usr/bin/{name}")))
        }
        fn path_exists(&self, path: &Path) -> bool {
            self.exists.iter().any(|known| Path::new(known) == path)
        }
        fn run(&self, _argv: &[String], _timeout: Duration) -> Option<String> {
            self.command_output.clone()
        }
    }

    const NVIDIA_FILES: &[(&str, &str)] = &[
        (
            "/etc/os-release",
            "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.4 LTS\"\n",
        ),
        ("/sys/module/nvidia/version", "595.71.05\n"),
        (
            "/proc/modules",
            "nvidia_uvm 1 0 - Live\nnvidia 1 400 - Live\n",
        ),
        ("/proc/cpuinfo", "processor\t: 0\nprocessor\t: 1\n"),
        ("/proc/meminfo", "MemTotal:       32767952 kB\n"),
    ];

    const AMD_FILES: &[(&str, &str)] = &[
        ("/etc/os-release", "PRETTY_NAME=\"Ubuntu 24.04 LTS\"\n"),
        ("/proc/modules", "amdgpu 1 99 - Live\n"),
        ("/proc/cpuinfo", "processor\t: 0\n"),
        ("/proc/meminfo", "MemTotal:       16000000 kB\n"),
    ];

    #[test]
    fn nvidia_wayland_gnome() {
        let probe = FakeProbe::new(
            &[
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME"),
                ("XDG_SESSION_TYPE", "wayland"),
            ],
            NVIDIA_FILES,
        )
        .with_program("gamemoderun");
        let p = detect(&probe);
        assert_eq!(p.gpu_vendor, "nvidia");
        assert_eq!(p.gpu_driver, "595.71.05");
        assert_eq!(p.gpu_name, "NVIDIA GPU");
        assert_eq!(p.desktop, "GNOME");
        assert_eq!(p.session, "wayland");
        assert_eq!(p.distro, "Ubuntu 24.04.4 LTS");
        assert_eq!(p.cpu_threads, 2);
        assert_eq!(p.ram_gb, 31);
        assert!(p.has_gamemode);
        assert!(!p.has_mangohud);
    }

    #[test]
    fn nvidia_smi_supplies_the_card_name() {
        let probe = FakeProbe::new(&[], NVIDIA_FILES)
            .with_program("nvidia-smi")
            .with_command_output("NVIDIA GeForce RTX 4070\n");
        assert_eq!(detect(&probe).gpu_name, "NVIDIA GeForce RTX 4070");
    }

    #[test]
    fn a_failing_nvidia_smi_falls_back_to_a_generic_name() {
        let probe = FakeProbe::new(&[], NVIDIA_FILES).with_program("nvidia-smi");
        assert_eq!(detect(&probe).gpu_name, "NVIDIA GPU");
    }

    #[test]
    fn amd_x11() {
        let probe = FakeProbe::new(
            &[("XDG_CURRENT_DESKTOP", "KDE"), ("XDG_SESSION_TYPE", "x11")],
            AMD_FILES,
        );
        let p = detect(&probe);
        assert_eq!(p.gpu_vendor, "amd");
        assert_eq!(p.desktop, "KDE");
        assert_eq!(p.session, "x11");
        assert!(!p.has_gamemode);
    }

    #[test]
    fn wayland_socket_fallback() {
        let probe = FakeProbe::new(&[("XDG_RUNTIME_DIR", "/run/user/1000")], AMD_FILES)
            .with_path("/run/user/1000/wayland-0");
        assert_eq!(detect(&probe).session, "wayland");
    }

    #[test]
    fn the_runtime_dir_is_derived_from_the_uid_when_unset() {
        // Run from cron or a user unit there may be no XDG_RUNTIME_DIR to
        // inherit, so the uid comes from /proc rather than a libc call.
        let mut files: Vec<(&str, &str)> = AMD_FILES.to_vec();
        files.push((
            "/proc/self/status",
            "Name:\tsteamtrain\nUid:\t1000\t1000\t1000\t1000\n",
        ));
        let probe = FakeProbe::new(&[], &files).with_path("/run/user/1000/wayland-0");
        assert_eq!(detect(&probe).session, "wayland");
    }

    #[test]
    fn x11_falls_back_to_display() {
        let probe = FakeProbe::new(&[("DISPLAY", ":0")], AMD_FILES);
        assert_eq!(detect(&probe).session, "x11");
    }

    #[test]
    fn an_empty_display_is_not_a_session() {
        let probe = FakeProbe::new(&[("DISPLAY", "")], AMD_FILES);
        assert_eq!(detect(&probe).session, "unknown");
    }

    #[test]
    fn unknown_everything() {
        let probe = FakeProbe::new(&[], &[]);
        let p = detect(&probe);
        assert_eq!(p.gpu_vendor, "unknown");
        assert_eq!(p.gpu_name, "");
        assert_eq!(p.session, "unknown");
        assert_eq!(p.desktop, "unknown");
        assert_eq!(p.distro, "unknown");
        assert_eq!(p.kernel, "unknown");
        assert_eq!(p.ram_gb, 0);
        assert!(p.cpu_threads > 0);
    }

    #[test]
    fn intel_from_loaded_modules() {
        let files = &[
            ("/proc/modules", "i915 1 2 - Live\n"),
            ("/proc/meminfo", ""),
            ("/proc/cpuinfo", ""),
        ];
        assert_eq!(detect(&FakeProbe::new(&[], files)).gpu_vendor, "intel");
    }

    #[test]
    fn radeon_and_xe_are_recognised_too() {
        let radeon = &[("/proc/modules", "radeon 1 2 - Live\n")];
        assert_eq!(detect(&FakeProbe::new(&[], radeon)).gpu_vendor, "amd");
        let xe = &[("/proc/modules", "xe 1 2 - Live\n")];
        assert_eq!(detect(&FakeProbe::new(&[], xe)).gpu_vendor, "intel");
    }

    #[test]
    fn the_profile_serializes_with_every_wire_field() {
        let value = serde_json::to_value(detect(&FakeProbe::new(&[], AMD_FILES))).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec![
                "distro",
                "kernel",
                "desktop",
                "session",
                "gpu_vendor",
                "gpu_name",
                "gpu_driver",
                "cpu_threads",
                "ram_gb",
                "has_gamemode",
                "has_mangohud",
                "has_gamescope",
            ]
        );
    }

    #[test]
    fn real_machine_smoke() {
        let p = detect(&RealProbe);
        assert!(["nvidia", "amd", "intel", "unknown"].contains(&p.gpu_vendor.as_str()));
        assert!(p.cpu_threads > 0);
    }
}
