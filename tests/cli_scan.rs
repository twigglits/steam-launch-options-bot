mod support;

use std::path::{Path, PathBuf};

use support::{make_manifest, make_steam_root, make_user, Cli};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = make_steam_root(tmp.path());
        make_manifest(&root, "100", "Fixture Game", "FixtureGame");
        make_user(&root, "111");
        Fixture {
            root,
            config: tmp.path().join("config.json"),
            state: tmp.path().join("state"),
            _tmp: tmp,
        }
    }

    fn args<'a>(&'a self, command: &[&'a str]) -> Vec<String> {
        let mut argv: Vec<String> = command.iter().map(|arg| arg.to_string()).collect();
        argv.extend([
            "--steam-root".to_string(),
            self.root.display().to_string(),
            "--config".to_string(),
            self.config.display().to_string(),
            "--state-dir".to_string(),
            self.state.display().to_string(),
        ]);
        argv
    }

    fn run(&self, cli: &Cli, command: &[&str]) -> support::Run {
        let argv = self.args(command);
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        cli.run(&borrowed, "")
    }
}

#[test]
fn scan_lists_a_game_and_its_proposal() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["scan"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("Fixture Game"), "got {}", run.out);
    assert!(run.out.contains("%command%"), "got {}", run.out);
    assert!(run.out.contains("mesa_glthread=true"), "got {}", run.out);
}

#[test]
fn scan_reports_no_games_plainly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    make_user(&root, "111");
    let cli = Cli::new("amd");
    let run = cli.run(
        &[
            "scan",
            "--steam-root",
            &root.display().to_string(),
            "--config",
            &tmp.path().join("config.json").display().to_string(),
            "--state-dir",
            &tmp.path().join("state").display().to_string(),
        ],
        "",
    );

    assert_eq!(run.code, 0);
    assert!(run
        .out
        .contains("No installed games found on mounted libraries."));
}

#[test]
fn scan_json_emits_profile_game_change_and_result() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("nvidia"), &["scan", "--json"]);

    assert_eq!(run.code, 0);
    assert_eq!(run.of_kind("profile").len(), 1);
    assert_eq!(run.of_kind("game").len(), 1);
    assert_eq!(run.of_kind("change").len(), 1);

    let profile = &run.of_kind("profile")[0];
    assert_eq!(profile["gpu_vendor"], "nvidia");
    assert_eq!(profile["session"], "wayland");
    assert_eq!(profile["distro"], "Arch Linux");

    let game = &run.of_kind("game")[0];
    assert_eq!(game["appid"], "100");
    assert_eq!(game["name"], "Fixture Game");
    assert_eq!(game["runtime"], "native");

    let result = run.result();
    assert_eq!(result["ok"], true);
    assert_eq!(result["outcome"], "ok");
    assert_eq!(result["steam_running"], false);
}

#[test]
fn every_record_carries_the_wire_version() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);
    assert!(run.records().iter().all(|record| record["v"] == 1));
}

#[test]
fn counts_name_every_action_even_at_zero() {
    // A client reads a missing key as "unknown"; zero has to be explicit.
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);

    let counts = &run.result()["counts"];
    assert_eq!(counts["set"], 1);
    assert_eq!(counts["skip-user-set"], 0);
    assert_eq!(counts["skip-unchanged"], 0);
    assert_eq!(counts["excluded"], 0);
}

#[test]
fn an_excluded_appid_still_gets_a_change_record() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, r#"{"exclude": ["100"]}"#).unwrap();

    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);

    let changes = run.of_kind("change");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["action"], "excluded");
    assert_eq!(changes[0]["appid"], "100");
    assert_eq!(changes[0]["user"], "111");
    assert_eq!(run.result()["counts"]["excluded"], 1);
}

#[test]
fn a_config_gpu_vendor_override_wins_over_detection() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, r#"{"gpu_vendor": "nvidia"}"#).unwrap();

    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);

    assert_eq!(run.of_kind("profile")[0]["gpu_vendor"], "nvidia");
}

#[test]
fn an_unrecognised_gpu_vendor_warns_on_stderr_and_autodetects() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, r#"{"gpu_vendor": "banana"}"#).unwrap();

    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);

    assert_eq!(run.of_kind("profile")[0]["gpu_vendor"], "amd");
    assert!(
        run.err
            .contains("WARNING: ignoring unrecognized gpu_vendor 'banana'; using autodetection"),
        "got {}",
        run.err
    );
    // Nothing but records may reach stdout while --json is active.
    assert!(!run.out.contains("WARNING"), "got {}", run.out);
}

#[test]
fn status_with_nothing_managed_says_so() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["status"]);

    assert_eq!(run.code, 0);
    assert!(run
        .out
        .contains("No launch options are currently managed by this tool."));
}

#[test]
fn status_json_reports_config_existence_without_creating_the_file() {
    // Reading status must not be what makes the first-run screen stop
    // appearing, so config_exists is probed rather than loaded.
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["status", "--json"]);

    let result = run.result();
    assert_eq!(result["config_exists"], false);
    assert!(!fixture.config.exists(), "status created the config file");
    assert_eq!(result["managed"], serde_json::json!({}));
}

#[test]
fn status_json_reports_config_existence_once_it_is_there() {
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["scan"]); // creates the config
    let run = fixture.run(&Cli::new("amd"), &["status", "--json"]);
    assert_eq!(run.result()["config_exists"], true);
}

#[test]
fn an_explicit_steam_root_is_taken_at_its_word_even_if_empty() {
    // The no-steam-root guardrail fires only when autodetection fails, which
    // depends on HOME and so is covered in tests/e2e.rs against a child
    // process with a controlled environment.
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nowhere");
    let run = Cli::new("amd").run(
        &[
            "scan",
            "--steam-root",
            &missing.display().to_string(),
            "--config",
            &tmp.path().join("config.json").display().to_string(),
            "--state-dir",
            &tmp.path().join("state").display().to_string(),
            "--json",
        ],
        "",
    );

    assert_eq!(run.code, 0);
    assert_eq!(run.result()["outcome"], "ok");
    assert_eq!(run.of_kind("game").len(), 0);
}

#[test]
fn invalid_config_json_is_a_config_invalid_guardrail() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, "{not json").unwrap();

    let run = fixture.run(&Cli::new("amd"), &["scan", "--json"]);

    assert_eq!(run.code, 1);
    let result = run.result();
    assert_eq!(result["outcome"], "error");
    assert_eq!(result["guardrail"], "config-invalid");
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("delete it to regenerate defaults"));
}

#[test]
fn invalid_config_json_is_an_error_line_in_text_mode() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, "{not json").unwrap();

    let run = fixture.run(&Cli::new("amd"), &["scan"]);

    assert_eq!(run.code, 1);
    assert!(run.err.starts_with("ERROR: "), "got {}", run.err);
    assert_eq!(run.out, "", "nothing on stdout when the run fails");
}

#[test]
fn a_corrupt_state_file_is_reported_not_silently_ignored() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(&fixture.state).unwrap();
    std::fs::write(fixture.state.join("state.json"), "{not json").unwrap();

    let run = fixture.run(&Cli::new("amd"), &["status", "--json"]);

    assert_eq!(run.code, 1);
    let result = run.result();
    assert_eq!(result["outcome"], "error");
    assert!(result["message"].as_str().unwrap().contains("state.json"));
    // Not a config problem, so it carries no config-invalid code.
    assert!(result.get("guardrail").is_none());
}

#[test]
fn the_json_flag_is_accepted_after_subcommand_flags() {
    // The desktop interface builds [core, *args, "--json"], so this ordering
    // is the contract.
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["apply", "--dry-run"]);
    assert_eq!(run.code, 0);

    let argv = fixture.args(&["apply", "--dry-run"]);
    let mut borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    borrowed.push("--json");
    let run = Cli::new("amd").run(&borrowed, "");

    assert_eq!(run.code, 0);
    assert_eq!(run.result()["dry_run"], true);
}

#[test]
fn a_usage_error_exits_two_and_writes_to_stderr() {
    let cli = Cli::new("amd");
    let run = cli.run(&["not-a-command"], "");

    assert_eq!(run.code, 2);
    assert!(!run.err.is_empty());
    assert_eq!(run.out, "");
}

#[test]
fn version_prints_name_and_version_only() {
    // steamtrain_gui/client.py:core_version() splits this on whitespace and
    // takes the last field.
    let run = Cli::new("amd").run(&["--version"], "");

    assert_eq!(run.code, 0);
    let parts: Vec<&str> = run.out.split_whitespace().collect();
    assert_eq!(parts, vec!["steamtrain", steamtrain::VERSION]);
}

#[test]
fn progress_records_bracket_a_long_run() {
    let tmp = tempfile::tempdir().unwrap();
    let root = make_steam_root(tmp.path());
    for index in 0..120 {
        make_manifest(
            &root,
            &format!("{index:04}"),
            &format!("Game {index}"),
            &format!("Dir{index}"),
        );
    }
    make_user(&root, "111");

    let cli = Cli::new("amd");
    let run = cli.run(
        &[
            "scan",
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

    let progress = run.of_kind("progress");
    assert!(
        progress.len() >= 3,
        "got {} progress records",
        progress.len()
    );
    let last = progress.last().unwrap();
    assert_eq!(last["done"], 120);
    assert_eq!(last["total"], 120);
}

#[test]
fn the_steam_root_flag_is_taken_at_its_word() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["scan"]);
    assert!(run.out.contains(&fixture.root.display().to_string()));
    assert!(Path::new(&fixture.root).is_dir());
}
