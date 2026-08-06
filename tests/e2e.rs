//! The process contract the desktop interface actually depends on.
//!
//! The other suites drive `cli::main` in process. These spawn the real binary,
//! which is the only way to assert on exit status, stream separation, and
//! anything that depends on the process environment.

mod support;

use std::path::Path;
use std::process::Command;

use support::{make_manifest, make_steam_root, make_user};

const BIN: &str = env!("CARGO_BIN_EXE_steamtrain");

struct Fixture {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let root = make_steam_root(&home);
        make_manifest(&root, "100", "Fixture Game", "FixtureGame");
        make_user(&root, "111");
        Fixture {
            home,
            root,
            _tmp: tmp,
        }
    }

    /// A run with HOME pointed at the fixture, so autodetection and the
    /// default config and state paths stay inside it.
    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command.env("HOME", &self.home);
        command
    }

    fn locations(&self, command: &mut Command) {
        command
            .arg("--steam-root")
            .arg(&self.root)
            .arg("--config")
            .arg(self.home.join("config.json"))
            .arg("--state-dir")
            .arg(self.home.join("state"));
    }
}

fn records(stdout: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|err| panic!("not JSON: {line:?} ({err})"))
        })
        .collect()
}

#[test]
fn version_is_exactly_name_space_version() {
    // steamtrain_gui/client.py:core_version() splits this on whitespace and
    // takes the last field.
    let out = Command::new(BIN).arg("--version").output().unwrap();

    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let parts: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(parts, vec!["steamtrain", steamtrain::VERSION]);
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let out = Command::new(BIN).arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(!out.stdout.is_empty());
    assert!(out.stderr.is_empty());
}

#[test]
fn a_usage_error_exits_two_on_stderr() {
    let out = Command::new(BIN).arg("not-a-command").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "usage errors do not touch stdout");
    assert!(!out.stderr.is_empty());
}

#[test]
fn json_mode_puts_only_records_on_stdout() {
    let fixture = Fixture::new();
    let mut command = fixture.command();
    command.arg("status");
    fixture.locations(&mut command);
    let out = command.arg("--json").output().unwrap();

    assert_eq!(out.status.code(), Some(0));
    let records = records(&out.stdout);
    assert!(!records.is_empty());
    assert!(records.iter().all(|record| record["v"] == 1));
    assert_eq!(records.last().unwrap()["kind"], "result");
    assert_eq!(
        records.iter().filter(|r| r["kind"] == "result").count(),
        1,
        "exactly one result record"
    );
}

#[test]
fn the_json_flag_is_accepted_after_subcommand_flags() {
    // The desktop interface builds [core, *args, "--json"], so this ordering
    // is the contract.
    let fixture = Fixture::new();
    let mut command = fixture.command();
    command.args(["apply", "--dry-run"]);
    fixture.locations(&mut command);
    let out = command.arg("--json").output().unwrap();

    assert_eq!(out.status.code(), Some(0));
    let records = records(&out.stdout);
    let last = records.last().unwrap();
    assert_eq!(last["kind"], "result");
    assert_eq!(last["dry_run"], true);
}

#[test]
fn a_full_apply_and_revert_round_trip_through_the_binary() {
    let fixture = Fixture::new();

    let mut apply = fixture.command();
    apply.arg("apply");
    fixture.locations(&mut apply);
    assert_eq!(apply.output().unwrap().status.code(), Some(0));

    let localconfig = fixture.root.join("userdata/111/config/localconfig.vdf");
    assert!(support::current_options(&localconfig, "100").contains("%command%"));

    let mut revert = fixture.command();
    revert.arg("revert");
    fixture.locations(&mut revert);
    assert_eq!(revert.output().unwrap().status.code(), Some(0));
    assert_eq!(support::current_options(&localconfig, "100"), "");
}

#[test]
fn no_steam_installation_is_a_guardrail_not_a_crash() {
    // Autodetection depends on HOME, which only a child process can be given
    // safely - the in-process suites share one environment across threads.
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .env("HOME", tmp.path())
        .args(["scan", "--json"])
        .arg("--config")
        .arg(tmp.path().join("config.json"))
        .arg("--state-dir")
        .arg(tmp.path().join("state"))
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
    let records = records(&out.stdout);
    let result = records.last().unwrap();
    assert_eq!(result["kind"], "result");
    assert_eq!(result["outcome"], "error");
    assert_eq!(result["guardrail"], "no-steam-root");
}

#[test]
fn no_steam_installation_is_an_error_line_in_text_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .env("HOME", tmp.path())
        .arg("scan")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "ERROR: no Steam installation found"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn the_default_config_and_state_paths_hang_off_home() {
    let fixture = Fixture::new();
    let out = fixture
        .command()
        .arg("apply")
        .arg("--steam-root")
        .arg(&fixture.root)
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(fixture
        .home
        .join(".config/steamtrain/config.json")
        .is_file());
    assert!(fixture
        .home
        .join(".local/state/steamtrain/state.json")
        .is_file());
}

#[test]
fn doctor_is_quiet_on_a_developer_checkout() {
    // A packaged install is what makes a legacy one a conflict; without one
    // there is nothing being shadowed. The exit-2-on-findings path is covered
    // in tests/cli_doctor.rs, which can point doctor at a fixture home.
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .env("HOME", tmp.path())
        .arg("doctor")
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("No problems found."));
}

#[test]
fn a_stale_pid_file_does_not_block_a_write() {
    // is_steam_running trusts /proc/<pid>/comm, not the pid file alone, so a
    // leftover steam.pid from a crashed client must not stop the timer
    // forever. The blocked-run path itself is covered in tests/cli_apply.rs,
    // which can force the guardrail.
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("steam.pid"), "999999\n").unwrap();

    let mut command = fixture.command();
    command.arg("apply");
    fixture.locations(&mut command);
    let out = command.arg("--json").output().unwrap();

    assert_eq!(out.status.code(), Some(0));
    let records = records(&out.stdout);
    assert_eq!(records.last().unwrap()["outcome"], "ok");
}

#[test]
fn the_binary_needs_no_arguments_to_show_usage() {
    let out = Command::new(BIN).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage"));
}

#[test]
fn every_subcommand_accepts_its_own_flags() {
    // A cheap guard that the parser shape did not drift: each of these must
    // parse, whatever it then does.
    let fixture = Fixture::new();
    for args in [
        vec!["scan", "--json"],
        vec!["apply", "--dry-run", "--json"],
        vec!["status", "--json"],
        vec!["revert", "--json"],
    ] {
        let mut command = fixture.command();
        command.args(&args);
        fixture.locations(&mut command);
        let out = command.output().unwrap();
        assert_ne!(
            out.status.code(),
            Some(2),
            "{args:?} failed to parse: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    for args in [
        vec!["setup", "--gpu-vendor", "auto", "--json"],
        vec!["doctor", "--json"],
        vec!["doctor", "--fix", "--dry-run", "--json"],
    ] {
        let out = fixture.command().args(&args).output().unwrap();
        assert_ne!(
            out.status.code(),
            Some(2),
            "{args:?} failed to parse: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_steam_root_flag_survives_a_path_with_spaces() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("a home with spaces");
    std::fs::create_dir_all(&home).unwrap();
    let root = make_steam_root(&home);
    make_manifest(&root, "100", "Fixture Game", "FixtureGame");
    make_user(&root, "111");

    let out = Command::new(BIN)
        .env("HOME", &home)
        .arg("scan")
        .arg("--steam-root")
        .arg(&root)
        .arg("--config")
        .arg(home.join("config.json"))
        .arg("--state-dir")
        .arg(home.join("state"))
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Fixture Game"));
}

#[test]
fn the_binary_is_where_cargo_says_it_is() {
    assert!(Path::new(BIN).is_file(), "{BIN}");
}
