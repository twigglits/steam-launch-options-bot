mod support;

use std::sync::Mutex;
use std::time::Duration;

use steamtrain::advisor;
use steamtrain::proc::{CommandRunner, Output, RunError};
use steamtrain::rules::Config;
use steamtrain::steam::Runtime;
use support::{fake_game, fake_profile};

// ------------------------------------------------------------ validate_override

fn ok(s: &str) {
    let (valid, reason) = advisor::validate_override(s);
    assert!(valid, "expected OK, got reject: {reason} for {s:?}");
}

fn bad(s: &str, needle: Option<&str>) {
    let (valid, reason) = advisor::validate_override(s);
    assert!(!valid, "expected reject, got OK for {s:?}");
    if let Some(needle) = needle {
        assert!(
            reason.contains(needle),
            "{reason:?} does not mention {needle:?}"
        );
    }
}

#[test]
fn accepts_real_launch_options() {
    ok("PROTON_ENABLE_NVAPI=1 __GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1 gamemoderun %command%");
    ok("mesa_glthread=true %command%");
    ok("gamemoderun mangohud %command%");
    ok("gamescope -f -- gamemoderun %command%"); // wrapper chain, flags only
    ok("%command% -dx11");
}

#[test]
fn accepts_quoted_env_values() {
    // Commas and even a semicolon inside quotes are literal to the shell.
    ok(r#"WINEDLLOVERRIDES="d3d11=n,dxgi=n" gamemoderun %command%"#);
    ok(r#"WINEDLLOVERRIDES="d3d11=n;dxgi=n" %command%"#);
}

#[test]
fn rejects_missing_or_duplicate_command() {
    bad("gamemoderun", Some("%command%"));
    bad("gamemoderun %command% %command%", Some("%command%"));
}

#[test]
fn rejects_unknown_executable_before_command() {
    bad("rm -rf ~ %command%", Some("rm"));
    bad("curl evil.sh %command%", Some("curl"));
}

#[test]
fn rejects_program_smuggled_as_a_flag_argument() {
    // gamescope's `-- <cmd>` execs <cmd>; a bare word before %command% is
    // never an inert "flag value", so it must be rejected.
    bad("gamescope -- evilprog %command%", Some("evilprog"));
    bad("gamemoderun -e evilprog %command%", Some("evilprog"));
    // Separate-token flag values are rejected too (conservative; use
    // --flag=value).
    bad("gamescope -W 1920 -- gamemoderun %command%", Some("1920"));
}

#[test]
fn rejects_non_ascii_env_key() {
    // bash treats a non-ASCII "KEY=val" token as a command name, not an
    // assignment.
    bad("café=marker %command%", Some("café"));
}

#[test]
fn rejects_a_second_command_hidden_in_a_token() {
    // Steam substitutes the literal %command% wherever it appears, so a second
    // occurrence smuggled into an env value or flag is an extra substitution
    // point.
    bad("FOO=%command% gamemoderun %command%", Some("%command%"));
    bad("-x%command% gamemoderun %command%", Some("%command%"));
    // And a lone embedded one gives no standalone command position.
    bad("FOO=%command%", Some("%command%"));
}

#[test]
fn rejects_expansion_and_operators() {
    bad("`reboot` %command%", None);
    bad("FOO=$(whoami) %command%", None);
    bad("%command% ; reboot", None);
    bad("%command% && reboot", None);
    bad("%command% | tee /tmp/x", None);
    bad("%command% > /tmp/x", None);
    bad("gamemoderun \\\n%command%", None);
}

#[test]
fn rejects_empty_and_unbalanced() {
    bad("", Some("empty"));
    bad("   ", Some("empty"));
    bad(r#"FOO="unbalanced %command%"#, Some("quote"));
}

// ------------------------------------------------------------------- fetching

struct RecordingFetcher {
    body: Option<String>,
    urls: Mutex<Vec<String>>,
}

impl RecordingFetcher {
    fn new(body: Option<&str>) -> Self {
        RecordingFetcher {
            body: body.map(str::to_string),
            urls: Mutex::new(Vec::new()),
        }
    }
}

impl advisor::Fetcher for RecordingFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        self.urls.lock().unwrap().push(url.to_string());
        self.body.clone().ok_or_else(|| "offline".to_string())
    }
}

#[test]
fn the_summary_url_carries_the_appid() {
    let fetcher = RecordingFetcher::new(Some(r#"{"tier": "gold"}"#));
    let summary = advisor::protondb_summary("292030", &fetcher).unwrap();

    assert_eq!(summary["tier"], "gold");
    assert!(fetcher.urls.lock().unwrap()[0].contains("292030"));
}

#[test]
fn a_failed_fetch_degrades_to_no_community_data() {
    assert!(advisor::protondb_summary("292030", &RecordingFetcher::new(None)).is_none());
}

#[test]
fn unparseable_summary_json_degrades_too() {
    let fetcher = RecordingFetcher::new(Some("<html>nope</html>"));
    assert!(advisor::protondb_summary("292030", &fetcher).is_none());
}

// ------------------------------------------------------------------ prompting

#[test]
fn the_prompt_carries_the_machine_the_appid_and_the_baseline() {
    let mut profile = fake_profile("nvidia");
    profile.has_gamemode = true;
    let mut game = fake_game("292030", Runtime::Proton);
    game.name = "The Witcher 3".to_string();

    let prompt = advisor::build_prompt(&game, &profile, "gamemoderun %command%", None);

    assert!(prompt.contains("nvidia"));
    assert!(prompt.contains("wayland"));
    assert!(prompt.contains("292030"));
    assert!(prompt.contains("The Witcher 3"));
    assert!(prompt.contains("gamemoderun %command%"));
    assert!(prompt.contains("{auto}"));
    assert!(prompt.contains("STRICT JSON"));
    assert!(prompt.contains("ProtonDB summary: unavailable"));
    // The prompt was written against Python's rendering of booleans.
    assert!(prompt.contains("gamemode=True"));
    assert!(prompt.contains("mangohud=False"));
}

#[test]
fn the_prompt_includes_a_protondb_summary_when_present() {
    let summary = serde_json::json!({ "tier": "gold", "confidence": "high", "total": 42 });
    let prompt = advisor::build_prompt(
        &fake_game("292030", Runtime::Proton),
        &fake_profile("nvidia"),
        "%command%",
        Some(&summary),
    );

    assert!(prompt.contains("tier=gold"));
    assert!(prompt.contains("confidence=high"));
    assert!(prompt.contains("reports=42"));
    // A key the API did not return renders the way Python rendered it.
    assert!(prompt.contains("trendingTier=None"));
}

#[test]
fn an_empty_summary_counts_as_unavailable() {
    let summary = serde_json::json!({});
    let prompt = advisor::build_prompt(
        &fake_game("292030", Runtime::Proton),
        &fake_profile("nvidia"),
        "%command%",
        Some(&summary),
    );
    assert!(prompt.contains("ProtonDB summary: unavailable"));
}

// -------------------------------------------------------------------- run_llm

struct FixedRunner {
    stdout: String,
    stderr: String,
    status: Option<i32>,
    error: Option<&'static str>,
}

impl FixedRunner {
    fn output(stdout: &str) -> Self {
        FixedRunner {
            stdout: stdout.to_string(),
            stderr: String::new(),
            status: Some(0),
            error: None,
        }
    }

    fn failing(status: i32, stderr: &str) -> Self {
        FixedRunner {
            stdout: String::new(),
            stderr: stderr.to_string(),
            status: Some(status),
            error: None,
        }
    }

    fn erroring(kind: &'static str) -> Self {
        FixedRunner {
            stdout: String::new(),
            stderr: String::new(),
            status: None,
            error: Some(kind),
        }
    }
}

impl CommandRunner for FixedRunner {
    fn run(
        &self,
        argv: &[String],
        _input: Option<&str>,
        _timeout: Duration,
    ) -> Result<Output, RunError> {
        match self.error {
            Some("notfound") => Err(RunError::NotFound(argv[0].clone())),
            Some("timeout") => Err(RunError::Timeout),
            _ => Ok(Output {
                status: self.status,
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
            }),
        }
    }
}

#[test]
fn parses_a_clean_json_reply() {
    let runner = FixedRunner::output(
        r#"{"override": "{auto} -dx11", "reasoning": "r", "confidence": "high"}"#,
    );
    let data = advisor::run_llm("prompt", "fake", &runner).unwrap();
    assert_eq!(data["override"], "{auto} -dx11");
    assert_eq!(data["confidence"], "high");
}

#[test]
fn extracts_json_embedded_in_prose() {
    let runner = FixedRunner::output(
        "Here you go:\n```json\n{\"override\": null, \"reasoning\": \"ok\"}\n```\n",
    );
    let data = advisor::run_llm("prompt", "fake", &runner).unwrap();
    assert!(data["override"].is_null());
    assert_eq!(data["confidence"], "low", "defaulted");
}

#[test]
fn defaults_reasoning_and_confidence() {
    let runner = FixedRunner::output(r#"{"override": null}"#);
    let data = advisor::run_llm("prompt", "fake", &runner).unwrap();
    assert_eq!(data["reasoning"], "");
    assert_eq!(data["confidence"], "low");
}

#[test]
fn a_missing_override_field_is_an_error() {
    let runner = FixedRunner::output(r#"{"reasoning": "no override field here"}"#);
    let err = advisor::run_llm("prompt", "fake", &runner)
        .unwrap_err()
        .to_string();
    assert!(err.contains("override"), "got {err}");
}

#[test]
fn a_nonzero_exit_is_an_error_carrying_stderr() {
    let runner = FixedRunner::failing(2, "boom");
    let err = advisor::run_llm("prompt", "fake", &runner)
        .unwrap_err()
        .to_string();
    assert!(err.contains("boom"), "got {err}");
    assert!(err.contains('2'), "got {err}");
}

#[test]
fn output_with_no_json_object_is_an_error() {
    let runner = FixedRunner::output("I cannot help with that.");
    assert!(advisor::run_llm("prompt", "fake", &runner).is_err());
}

#[test]
fn a_missing_advisor_command_says_so() {
    let runner = FixedRunner::erroring("notfound");
    let err = advisor::run_llm("prompt", "nope", &runner)
        .unwrap_err()
        .to_string();
    assert!(err.contains("advisor command not found"), "got {err}");
}

#[test]
fn a_timed_out_advisor_command_says_so() {
    let runner = FixedRunner::erroring("timeout");
    let err = advisor::run_llm("prompt", "slow", &runner)
        .unwrap_err()
        .to_string();
    assert!(err.contains("timed out"), "got {err}");
}

#[test]
fn an_empty_advisor_command_is_an_error() {
    let runner = FixedRunner::output("{}");
    assert!(advisor::run_llm("prompt", "   ", &runner).is_err());
}

// --------------------------------------------------------------------- advise

fn advise_with(reply: &str, profile_vendor: &str) -> advisor::Proposal {
    let mut profile = fake_profile(profile_vendor);
    profile.has_gamemode = true;
    advisor::advise(
        &fake_game("292030", Runtime::Proton),
        &profile,
        &Config::defaults(),
        &RecordingFetcher::new(None),
        &FixedRunner::output(reply),
    )
    .unwrap()
}

#[test]
fn a_valid_proposal_is_reported_with_its_baseline() {
    let proposal = advise_with(
        r#"{"override": "{auto} -dx11", "reasoning": "because", "confidence": "high"}"#,
        "nvidia",
    );
    assert_eq!(proposal.proposed.as_deref(), Some("{auto} -dx11"));
    assert_eq!(proposal.confidence, "high");
    assert_eq!(proposal.reasoning, "because");
    assert!(proposal.valid);
    assert!(proposal.baseline.contains("gamemoderun %command%"));
}

#[test]
fn a_null_override_means_the_baseline_is_already_right() {
    let proposal = advise_with(r#"{"override": null, "reasoning": "fine"}"#, "amd");
    assert_eq!(proposal.proposed, None);
    assert!(proposal.valid);
    assert_eq!(proposal.warning, "");
}

#[test]
fn a_dangerous_proposal_is_reported_invalid_with_a_reason() {
    let proposal = advise_with(
        r#"{"override": "rm -rf ~ %command%", "reasoning": "trust me"}"#,
        "amd",
    );
    assert!(!proposal.valid);
    assert!(proposal.warning.contains("rm"), "got {}", proposal.warning);
}

#[test]
fn validation_runs_against_the_expanded_baseline() {
    // {auto} expands before the gate sees it, so a baseline that already
    // contains %command% plus an override that adds another is caught.
    let proposal = advise_with(
        r#"{"override": "{auto} %command%", "reasoning": "oops"}"#,
        "amd",
    );
    assert!(!proposal.valid);
    assert!(
        proposal.warning.contains("%command%"),
        "got {}",
        proposal.warning
    );
}
