mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{make_steam_root, make_user, Cli};

fn legacy_install(home: &Path) {
    fs::create_dir_all(home.join(".local/lib/steamtrain")).unwrap();
    fs::write(home.join(".local/lib/steamtrain/marker"), "").unwrap();
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    fs::write(home.join(".local/bin/steamtrain"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(home.join(".config/systemd/user")).unwrap();
    fs::write(home.join(".config/systemd/user/steamtrain.service"), "").unwrap();
    fs::write(home.join(".config/systemd/user/steamtrain.timer"), "").unwrap();
}

fn packaged(dir: &Path) -> PathBuf {
    let bin = dir.join("usr-bin-steamtrain");
    fs::write(&bin, "#!/bin/sh\n").unwrap();
    bin
}

/// A harness whose doctor sees `home` and believes a package is installed.
fn cli_seeing(home: &Path, packaged_bin: PathBuf) -> Cli {
    let mut cli = Cli::new("amd");
    cli.doctor.home = Some(home.to_path_buf());
    cli.doctor.packaged_bin = packaged_bin;
    cli
}

// ----------------------------------------------------------------- doctor

#[test]
fn a_clean_machine_reports_no_problems() {
    let tmp = tempfile::tempdir().unwrap();
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor"], "");

    assert_eq!(run.code, 0);
    assert!(run.out.contains("No problems found."), "got {}", run.out);
}

#[test]
fn findings_exit_two_and_name_every_path() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor"], "");

    assert_eq!(run.code, 2);
    assert!(run.out.contains("PROBLEM:"), "got {}", run.out);
    assert!(run.out.contains(".local/lib/steamtrain"), "got {}", run.out);
    assert!(
        run.out.contains("Run `steamtrain doctor --fix` to repair."),
        "got {}",
        run.out
    );
    assert!(
        tmp.path().join(".local/lib/steamtrain").exists(),
        "read-only"
    );
}

#[test]
fn fix_removes_the_legacy_install_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor", "--fix"], "");

    assert_eq!(run.code, 0);
    assert!(
        run.out
            .contains("Migrated. Configuration and state were left untouched."),
        "got {}",
        run.out
    );
    assert!(!tmp.path().join(".local/lib/steamtrain").exists());
    assert!(!tmp.path().join(".local/bin/steamtrain").exists());
}

#[test]
fn a_dry_run_fix_exits_two_and_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run =
        cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor", "--fix", "--dry-run"], "");

    assert_eq!(run.code, 2);
    assert!(run.out.contains("would remove"), "got {}", run.out);
    assert!(
        run.out.contains("dry-run: nothing was removed"),
        "got {}",
        run.out
    );
    assert!(tmp.path().join(".local/lib/steamtrain").exists());
}

#[test]
fn force_reports_without_a_packaged_install() {
    // install.sh --migrate: the user asked to clear the old install before a
    // package exists to shadow it.
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run = cli_seeing(tmp.path(), tmp.path().join("absent")).run(&["doctor", "--force"], "");

    assert_eq!(run.code, 2);
    assert!(run.out.contains("PROBLEM:"), "got {}", run.out);
}

#[test]
fn doctor_json_emits_findings_and_a_result() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor", "--json"], "");

    assert_eq!(run.code, 2);
    let findings = run.of_kind("finding");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["code"], "legacy-install-shadowing");
    assert_eq!(findings[0]["fixable"], true);
    assert_eq!(findings[0]["paths"].as_array().unwrap().len(), 4);

    let result = run.result();
    assert_eq!(result["ok"], false);
    assert_eq!(result["findings"], 1);
    assert_eq!(result["fixed"], 0);
}

#[test]
fn doctor_fix_json_reports_what_it_removed() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor", "--fix", "--json"], "");

    assert_eq!(run.code, 0);
    let result = run.result();
    assert_eq!(result["ok"], true);
    assert_eq!(result["fixed"], 4);
    assert_eq!(result["dry_run"], false);
    assert!(result["failed"].as_array().unwrap().is_empty());
    assert_eq!(result["removed"].as_array().unwrap().len(), 4);
}

#[test]
fn doctor_json_on_a_clean_machine_still_ends_with_a_result() {
    let tmp = tempfile::tempdir().unwrap();
    let run = cli_seeing(tmp.path(), packaged(tmp.path())).run(&["doctor", "--json"], "");

    assert_eq!(run.code, 0);
    let result = run.result();
    assert_eq!(result["ok"], true);
    assert_eq!(result["findings"], 0);
}

#[test]
fn a_shadowed_install_warns_on_stderr_and_the_command_still_runs() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");

    let cli = cli_seeing(tmp.path(), packaged(tmp.path()));
    let run = cli.run(
        &[
            "status",
            "--steam-root",
            &root.display().to_string(),
            "--config",
            &tmp.path().join("config.json").display().to_string(),
            "--state-dir",
            &tmp.path().join("state").display().to_string(),
        ],
        "",
    );

    assert_eq!(run.code, 0, "the warning does not stop the command");
    assert!(run.err.contains("PROBLEM:"), "got stderr {}", run.err);
    assert!(
        run.err
            .contains("The packaged install is not the code being run."),
        "got stderr {}",
        run.err
    );
    assert!(
        run.err.contains("steamtrain doctor --fix"),
        "got {}",
        run.err
    );
    assert!(
        run.out.contains("No launch options"),
        "got stdout {}",
        run.out
    );
}

#[test]
fn the_legacy_warning_becomes_finding_records_in_json_mode() {
    let tmp = tempfile::tempdir().unwrap();
    legacy_install(tmp.path());
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");

    let cli = cli_seeing(tmp.path(), packaged(tmp.path()));
    let run = cli.run(
        &[
            "status",
            "--steam-root",
            &root.display().to_string(),
            "--config",
            &tmp.path().join("config.json").display().to_string(),
            "--state-dir",
            &tmp.path().join("state").display().to_string(),
            "--json",
        ],
        "",
    );

    assert_eq!(run.of_kind("finding").len(), 1);
    // Nothing but records reached stdout, and the stream still terminates.
    run.result();
}
