mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{make_manifest, make_steam_root, make_user, Cli, FakeRunner, Run};

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

// ----------------------------------------------------------------- advise

struct AdviseFixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl AdviseFixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = make_steam_root(tmp.path());
        make_manifest(&root, "100", "The Witcher 3", "Witcher3");
        make_manifest(&root, "200", "Portal", "Portal");
        make_manifest(&root, "300", "Portal 2", "Portal2");
        make_user(&root, "111");
        AdviseFixture {
            root,
            config: tmp.path().join("config.json"),
            state: tmp.path().join("state"),
            _tmp: tmp,
        }
    }

    fn run(&self, cli: &Cli, args: &[&str]) -> Run {
        let mut argv: Vec<String> = vec!["advise".to_string()];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv.extend([
            "--steam-root".to_string(),
            self.root.display().to_string(),
            "--config".to_string(),
            self.config.display().to_string(),
            "--state-dir".to_string(),
            self.state.display().to_string(),
        ]);
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        cli.run(&borrowed, "")
    }

    fn overrides(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(&self.config).unwrap_or_else(|_| "{}".to_string());
        let data: serde_json::Value = serde_json::from_str(&text).unwrap();
        data["overrides"].clone()
    }
}

fn advising(reply: &str) -> Cli {
    let mut cli = Cli::new("amd");
    cli.runner = FakeRunner::replying(reply);
    cli
}

#[test]
fn bare_advise_lists_installed_games() {
    let fixture = AdviseFixture::new();
    let run = fixture.run(&Cli::new("amd"), &[]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("3 installed game(s)"), "got {}", run.out);
    assert!(run.out.contains("The Witcher 3"), "got {}", run.out);
    // Sorted case-insensitively by name: Portal, Portal 2, The Witcher 3.
    let portal = run.out.find("Portal").unwrap();
    let witcher = run.out.find("The Witcher 3").unwrap();
    assert!(portal < witcher, "not sorted by name: {}", run.out);
}

#[test]
fn a_name_substring_selects_one_game() {
    let fixture = AdviseFixture::new();
    let cli = advising(r#"{"override": "{auto} -dx11", "reasoning": "why", "confidence": "high"}"#);
    let run = fixture.run(&cli, &["witcher"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("The Witcher 3"), "got {}", run.out);
    assert!(run.out.contains("-dx11"), "got {}", run.out);
    assert!(run.out.contains("confidence: high"), "got {}", run.out);
    assert!(run.out.contains("--write"), "got {}", run.out);
    // load_config creates the documented default file, so `overrides` exists
    // and is empty; the point is that reviewing wrote no entry into it.
    assert_eq!(fixture.overrides(), serde_json::json!({}));
}

#[test]
fn an_appid_selects_one_game() {
    let fixture = AdviseFixture::new();
    let cli = advising(r#"{"override": null, "reasoning": "fine"}"#);
    let run = fixture.run(&cli, &["100"]);

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("(LLM: baseline already appropriate)"),
        "got {}",
        run.out
    );
}

#[test]
fn an_ambiguous_name_exits_one_and_lists_the_candidates() {
    let fixture = AdviseFixture::new();
    let run = fixture.run(&Cli::new("amd"), &["portal"]);

    assert_eq!(run.code, 1);
    assert!(run.err.contains("matches 2 games"), "got {}", run.err);
    assert!(
        run.err.contains("200") && run.err.contains("300"),
        "got {}",
        run.err
    );
}

#[test]
fn an_unmatched_name_points_at_the_listing_command() {
    let fixture = AdviseFixture::new();
    let run = fixture.run(&Cli::new("amd"), &["quake"]);

    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("no installed game matches 'quake'"),
        "got {}",
        run.err
    );
    assert!(run.err.contains("steamtrain advise"), "got {}", run.err);
}

#[test]
fn write_saves_the_proposal_into_overrides() {
    let fixture = AdviseFixture::new();
    let cli = advising(r#"{"override": "{auto} -dx11", "reasoning": "why", "confidence": "high"}"#);
    let run = fixture.run(&cli, &["witcher", "--write"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("Saved overrides[100]"), "got {}", run.out);
    assert_eq!(fixture.overrides()["100"], "{auto} -dx11");
}

#[test]
fn a_rejected_proposal_exits_one_and_writes_nothing() {
    let fixture = AdviseFixture::new();
    let cli = advising(r#"{"override": "rm -rf ~ %command%", "reasoning": "trust me"}"#);
    let run = fixture.run(&cli, &["witcher", "--write"]);

    assert_eq!(run.code, 1);
    assert!(
        run.err.contains("REJECTED by safety check"),
        "got stderr {}",
        run.err
    );
    assert!(run.out.contains("Nothing written."), "got {}", run.out);
    assert_eq!(fixture.overrides(), serde_json::json!({}));
}

#[test]
fn an_llm_failure_exits_one_with_the_reason() {
    let fixture = AdviseFixture::new();
    let cli = advising("I cannot help with that.");
    let run = fixture.run(&cli, &["witcher"]);

    assert_eq!(run.code, 1);
    assert!(run.err.starts_with("ERROR: "), "got {}", run.err);
}

#[test]
fn no_installed_games_exits_one() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");

    let run = Cli::new("amd").run(
        &[
            "advise",
            "--steam-root",
            &root.display().to_string(),
            "--config",
            &tmp.path().join("config.json").display().to_string(),
            "--state-dir",
            &tmp.path().join("state").display().to_string(),
        ],
        "",
    );

    assert_eq!(run.code, 1);
    assert!(
        run.err
            .contains("No installed games found on mounted libraries."),
        "got {}",
        run.err
    );
}
