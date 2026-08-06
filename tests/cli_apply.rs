mod support;

use std::path::PathBuf;

use support::{current_options, make_manifest, make_steam_root, make_user, set_options, Cli, Run};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    localconfig: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = make_steam_root(tmp.path());
        make_manifest(&root, "100", "Fixture Game", "FixtureGame");
        let localconfig = make_user(&root, "111");
        Fixture {
            root,
            localconfig,
            config: tmp.path().join("config.json"),
            state: tmp.path().join("state"),
            _tmp: tmp,
        }
    }

    fn run(&self, cli: &Cli, command: &[&str]) -> Run {
        let mut argv: Vec<String> = command.iter().map(|arg| arg.to_string()).collect();
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

    fn options(&self) -> String {
        current_options(&self.localconfig, "100")
    }
}

#[test]
fn dry_run_writes_nothing() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.localconfig).unwrap();

    let run = fixture.run(&Cli::new("amd"), &["apply", "--dry-run"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("dry-run"), "got {}", run.out);
    assert!(run.out.contains("nothing touched"), "got {}", run.out);
    assert_eq!(std::fs::read(&fixture.localconfig).unwrap(), before);
}

#[test]
fn apply_writes_the_proposed_options() {
    let fixture = Fixture::new();

    let run = fixture.run(&Cli::new("amd"), &["apply"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("1 set, 0 skipped"), "got {}", run.out);
    assert_eq!(fixture.options(), "mesa_glthread=true %command%");
}

#[test]
fn apply_is_idempotent() {
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["apply"]);

    let run = fixture.run(&Cli::new("amd"), &["apply"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("0 set, 1 skipped"), "got {}", run.out);
    assert!(run.out.contains("[ok  ]"), "got {}", run.out);
}

#[test]
fn apply_keeps_a_value_a_human_set() {
    let fixture = Fixture::new();
    set_options(&fixture.localconfig, "100", "HUMAN %command%");

    let run = fixture.run(&Cli::new("amd"), &["apply"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("[KEEP]"), "got {}", run.out);
    assert!(
        run.out.contains("keeping human-set value: HUMAN %command%"),
        "got {}",
        run.out
    );
    assert_eq!(fixture.options(), "HUMAN %command%");
}

#[test]
fn a_blocked_run_exits_zero_and_reports_blocked() {
    // Steam being open is the expected case, not a failure: the timer must not
    // record one for it.
    let fixture = Fixture::new();
    let mut cli = Cli::new("amd");
    cli.steam_running = true;

    let run = fixture.run(&cli, &["apply", "--json"]);

    assert_eq!(run.code, 0);
    let result = run.result();
    assert_eq!(result["ok"], false);
    assert_eq!(result["outcome"], "blocked");
    assert_eq!(result["guardrail"], "steam-running");
    assert_eq!(result["written"], 0);
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("Close Steam and re-run"));
    assert_eq!(fixture.options(), "", "nothing written");
}

#[test]
fn a_blocked_run_is_a_note_in_text_mode() {
    let fixture = Fixture::new();
    let mut cli = Cli::new("amd");
    cli.steam_running = true;

    let run = fixture.run(&cli, &["apply"]);

    assert_eq!(run.code, 0);
    assert!(
        run.out.contains("NOTE: Steam is running"),
        "got {}",
        run.out
    );
}

#[test]
fn apply_json_reports_what_it_wrote() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["apply", "--json"]);

    assert_eq!(run.code, 0);
    let result = run.result();
    assert_eq!(result["outcome"], "ok");
    assert_eq!(result["written"], 1);
    assert_eq!(result["counts"]["set"], 1);
    assert!(result.get("dry_run").is_none(), "only a dry run says so");
}

#[test]
fn a_dry_run_says_so_and_reports_steam_state() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["apply", "--dry-run", "--json"]);

    let result = run.result();
    assert_eq!(result["dry_run"], true);
    assert_eq!(result["steam_running"], false);
    assert!(result.get("written").is_none(), "a dry run wrote nothing");
}

#[test]
fn revert_restores_managed_options_to_empty() {
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["apply"]);
    assert_eq!(fixture.options(), "mesa_glthread=true %command%");

    let run = fixture.run(&Cli::new("amd"), &["revert"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("1 reverted"), "got {}", run.out);
    assert_eq!(fixture.options(), "");
}

#[test]
fn revert_with_nothing_managed_says_so() {
    let fixture = Fixture::new();
    let run = fixture.run(&Cli::new("amd"), &["revert"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("Nothing to revert."), "got {}", run.out);
}

#[test]
fn revert_json_emits_no_game_records() {
    // Revert plans against state, which can hold appids that are no longer
    // installed; clients render those by appid.
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["apply"]);

    let run = fixture.run(&Cli::new("amd"), &["revert", "--json"]);

    assert!(run.of_kind("game").is_empty());
    assert!(run.of_kind("profile").is_empty());
    assert_eq!(run.of_kind("change").len(), 1);
    assert_eq!(run.result()["written"], 1);
}

#[test]
fn revert_leaves_a_value_a_human_changed() {
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["apply"]);
    set_options(&fixture.localconfig, "100", "HUMAN %command%");

    let run = fixture.run(&Cli::new("amd"), &["revert"]);

    assert_eq!(run.code, 0);
    assert!(run.out.contains("[KEEP]"), "got {}", run.out);
    assert_eq!(fixture.options(), "HUMAN %command%");
}

#[test]
fn a_blocked_revert_also_exits_zero() {
    let fixture = Fixture::new();
    fixture.run(&Cli::new("amd"), &["apply"]);

    let mut cli = Cli::new("amd");
    cli.steam_running = true;
    let run = fixture.run(&cli, &["revert", "--json"]);

    assert_eq!(run.code, 0);
    assert_eq!(run.result()["outcome"], "blocked");
    assert_eq!(fixture.options(), "mesa_glthread=true %command%");
}

#[test]
fn the_result_record_is_last_and_appears_once() {
    let fixture = Fixture::new();
    for command in [
        vec!["scan", "--json"],
        vec!["apply", "--dry-run", "--json"],
        vec!["apply", "--json"],
        vec!["status", "--json"],
        vec!["revert", "--json"],
    ] {
        let run = fixture.run(&Cli::new("amd"), &command);
        // result() asserts both properties.
        run.result();
    }
}

#[test]
fn an_override_reaches_the_written_value() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, r#"{"overrides": {"100": "{auto} -dx11"}}"#).unwrap();

    fixture.run(&Cli::new("amd"), &["apply"]);

    assert_eq!(fixture.options(), "mesa_glthread=true %command% -dx11");
}

#[test]
fn an_excluded_game_is_never_written() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, r#"{"exclude": ["100"]}"#).unwrap();

    let run = fixture.run(&Cli::new("amd"), &["apply", "--json"]);

    assert_eq!(run.result()["written"], 0);
    assert_eq!(fixture.options(), "");
}
