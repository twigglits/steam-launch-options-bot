use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use steamtrain::codes::Guardrail;
use steamtrain::doctor::{self, Options};
use steamtrain::proc::{CommandRunner, Output, RunError};

/// Records what it was asked to run, so the systemd call can be asserted on
/// without a session being present.
#[derive(Default)]
struct RecordingRunner {
    calls: std::sync::Mutex<Vec<Vec<String>>>,
}

impl CommandRunner for RecordingRunner {
    fn run(
        &self,
        argv: &[String],
        _input: Option<&str>,
        _timeout: Duration,
    ) -> Result<Output, RunError> {
        self.calls.lock().unwrap().push(argv.to_vec());
        Ok(Output {
            status: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn legacy_install(home: &Path) {
    fs::create_dir_all(home.join(".local/lib/steamtrain")).unwrap();
    fs::write(home.join(".local/lib/steamtrain/marker"), "").unwrap();
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    fs::write(home.join(".local/bin/steamtrain"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(home.join(".config/systemd/user")).unwrap();
    fs::write(home.join(".config/systemd/user/steamtrain.service"), "").unwrap();
    fs::write(home.join(".config/systemd/user/steamtrain.timer"), "").unwrap();
}

/// A stand-in for /usr/bin/steamtrain existing.
fn packaged(dir: &Path) -> PathBuf {
    let bin = dir.join("usr-bin-steamtrain");
    fs::write(&bin, "#!/bin/sh\n").unwrap();
    bin
}

fn options(home: &Path, packaged_bin: PathBuf) -> Options {
    Options {
        home: Some(home.to_path_buf()),
        path_env: Some("/nonexistent".to_string()),
        packaged_bin,
        force: false,
    }
}

#[test]
fn no_packaged_install_means_no_findings() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());

    let opts = options(tmp.path(), tmp.path().join("absent"));
    assert!(doctor::diagnose(&opts).is_empty());
}

#[test]
fn a_legacy_install_beside_a_packaged_one_is_a_finding() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());

    let findings = doctor::diagnose(&options(tmp.path(), packaged(tmp.path())));
    assert_eq!(findings.len(), 1);
    assert!(findings[0].fixable);
    assert_eq!(findings[0].code, Guardrail::LegacyInstallShadowing);
    assert!(findings[0].message.contains("not the code being run"));
    assert!(findings[0]
        .paths
        .iter()
        .any(|path| path.ends_with(".local/lib/steamtrain")));
    assert_eq!(findings[0].paths.len(), 4, "got {:?}", findings[0].paths);
}

#[test]
fn force_reports_a_legacy_install_with_no_package_present() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());

    let mut opts = options(tmp.path(), tmp.path().join("absent"));
    opts.force = true;
    assert_eq!(doctor::diagnose(&opts).len(), 1);
}

#[test]
fn a_clean_machine_has_no_findings() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(doctor::diagnose(&options(tmp.path(), packaged(tmp.path()))).is_empty());
}

#[test]
fn a_lone_binary_that_loses_the_path_lookup_is_inert() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".local/bin")).unwrap();
    fs::write(tmp.path().join(".local/bin/steamtrain"), "#!/bin/sh\n").unwrap();

    // PATH does not contain the legacy bin, so nothing is being shadowed.
    assert!(doctor::diagnose(&options(tmp.path(), packaged(tmp.path()))).is_empty());
}

#[test]
fn a_lone_binary_that_wins_the_path_lookup_is_a_finding() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join(".local/bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let legacy = bin_dir.join("steamtrain");
    fs::write(&legacy, "#!/bin/sh\n").unwrap();
    let mut perms = fs::metadata(&legacy).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&legacy, perms).unwrap();

    let mut opts = options(tmp.path(), packaged(tmp.path()));
    opts.path_env = Some(bin_dir.to_string_lossy().into_owned());

    let findings = doctor::diagnose(&opts);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].paths.len(), 1);
}

#[test]
fn a_dangling_legacy_symlink_still_counts() {
    // exists() follows symlinks, but a dangling ~/.local/bin/steamtrain still
    // wins the PATH lookup and still masks the package.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".config/systemd/user")).unwrap();
    std::os::unix::fs::symlink(
        tmp.path().join("gone"),
        tmp.path().join(".config/systemd/user/steamtrain.timer"),
    )
    .unwrap();

    let findings = doctor::diagnose(&options(tmp.path(), packaged(tmp.path())));
    assert_eq!(findings.len(), 1);
}

#[test]
fn migrate_removes_the_allowlist_and_reports_each_path() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());

    let (removed, failed) = doctor::migrate(tmp.path(), false, &RecordingRunner::default());

    assert!(failed.is_empty(), "got {failed:?}");
    assert_eq!(removed.len(), 4);
    assert!(removed.iter().all(|(_, detail)| detail == "removed"));
    assert!(!tmp.path().join(".local/lib/steamtrain").exists());
    assert!(!tmp.path().join(".local/bin/steamtrain").exists());
    assert!(!tmp
        .path()
        .join(".config/systemd/user/steamtrain.timer")
        .exists());
    assert!(!tmp
        .path()
        .join(".config/systemd/user/steamtrain.service")
        .exists());
}

#[test]
fn migrate_stops_the_timer_before_deleting_its_unit() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let runner = RecordingRunner::default();

    doctor::migrate(tmp.path(), false, &runner);

    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "disable".to_string(),
            "--now".to_string(),
            "steamtrain.timer".to_string(),
        ]
    );
}

#[test]
fn migrate_never_touches_config_or_state() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    fs::create_dir_all(tmp.path().join(".config/steamtrain")).unwrap();
    fs::write(tmp.path().join(".config/steamtrain/config.json"), "{}").unwrap();
    fs::create_dir_all(tmp.path().join(".local/state/steamtrain")).unwrap();
    fs::write(tmp.path().join(".local/state/steamtrain/state.json"), "{}").unwrap();

    doctor::migrate(tmp.path(), false, &RecordingRunner::default());

    assert!(tmp.path().join(".config/steamtrain/config.json").is_file());
    assert!(tmp
        .path()
        .join(".local/state/steamtrain/state.json")
        .is_file());
}

#[test]
fn a_dry_run_removes_nothing_and_starts_no_process() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let runner = RecordingRunner::default();

    let (removed, failed) = doctor::migrate(tmp.path(), true, &runner);

    assert!(failed.is_empty());
    assert_eq!(removed.len(), 4);
    assert!(removed.iter().all(|(_, detail)| detail == "would remove"));
    assert!(tmp.path().join(".local/lib/steamtrain").exists());
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[test]
fn migrate_on_a_clean_machine_does_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (removed, failed) = doctor::migrate(tmp.path(), false, &RecordingRunner::default());
    assert!(removed.is_empty());
    assert!(failed.is_empty());
}
