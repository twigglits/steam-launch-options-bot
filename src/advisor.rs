//! On-demand LLM advisor: propose a per-game launch-options override.
//!
//! The deterministic engine (rules/apply) is untouched; this only *proposes*
//! values for the existing `overrides` config, gated behind human approval.
//!
//! Steam substitutes %command% into the launch-options string and runs the
//! result through a shell, so any unquoted shell operator, or a $/backtick
//! expansion, in a proposed override is a command-injection vector. A
//! legitimate override is only environment assignments, known wrapper
//! programs, flags, and exactly one %command%; `validate_override` enforces
//! that shape and is the safety gate. Nothing is written without the user
//! re-running with --write.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::proc::{CommandRunner, RunError};
use crate::rules::{self, Config};
use crate::steam::Game;
use crate::sysinfo::SystemProfile;

/// Wrappers Steam may exec as the leading command. A wrapper's CLI must not
/// treat a following bare word as a subcommand to run (that word is not
/// re-validated) - vet that property before adding one here. `--` is handled
/// by rejecting any bare word before %command%, so gamescope's `-- <cmd>`
/// cannot smuggle a program.
pub const KNOWN_WRAPPERS: [&str; 10] = [
    "gamemoderun",
    "mangohud",
    "mangoapp",
    "gamescope",
    "prime-run",
    "primusrun",
    "optirun",
    "strangle",
    "obs-gamecapture",
    "umu-run",
];

/// Expansion/substitution/escape that must never appear, even inside quotes.
const EXPANSION: [char; 3] = ['`', '$', '\\'];

/// Shell operators that are only safe when quoted.
const OPERATORS: [char; 12] = [
    ';', '|', '&', '<', '>', '(', ')', '{', '}', '\n', '\r', '\0',
];

const PROTONDB_URL: &str = "https://www.protondb.com/api/v1/reports/summaries/{appid}.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const LLM_TIMEOUT: Duration = Duration::from_secs(300);

/// LLM invocation or output could not be used.
#[derive(Debug)]
pub struct AdvisorError(String);

impl fmt::Display for AdvisorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AdvisorError {}

// ---------------------------------------------------------------- validation

/// A POSIX assignment word: an ASCII identifier before the '='. bash treats a
/// non-ASCII "KEY=val" token as a command name, not an assignment, so a
/// Unicode-aware identifier check would be too lax for a security gate.
fn is_env_assign(token: &str) -> bool {
    let Some((key, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `s` with every '...'/"..." span removed, or None if a quote is unbalanced.
///
/// Used to check that shell metacharacters appear only inside quotes, where
/// the shell treats them literally - e.g. WINEDLLOVERRIDES="d3d11=n;dxgi=n".
fn strip_quoted(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            match chars[i + 1..].iter().position(|&other| other == c) {
                Some(offset) => i += offset + 2,
                None => return None, // unbalanced quote
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    Some(out)
}

/// (ok, reason) - reject launch strings that could execute unexpected code.
///
/// Steam runs the substituted launch string through a shell, so the gate is
/// two layers. Layer 1: no $/backtick/backslash anywhere, and no *unquoted*
/// shell operator (a metacharacter inside quotes is literal and allowed).
/// Layer 2: the literal text %command% must appear exactly once (Steam
/// substitutes every occurrence, so a second one hidden in an env value or
/// flag is an extra, ungated substitution point) and as a standalone token;
/// every token before it must be an env-assignment, an option flag, or a
/// known-safe wrapper - so the shell (and any wrapper it chains) execs nothing
/// unexpected. Input must already be {auto}-expanded.
///
/// A separate-token flag value (e.g. `-W 1920`) and an unknown wrapper are
/// rejected, since a bare word before %command% could otherwise be a program a
/// wrapper execs (e.g. `gamescope -- evilprog`). Use `--flag=value` form, or
/// add the rarity to overrides by hand. Likewise values needing `$`/`\` are
/// rejected. Upgrade path: a real shell grammar if that ever matters.
pub fn validate_override(s: &str) -> (bool, String) {
    if s.trim().is_empty() {
        return (false, "empty override".to_string());
    }
    for bad in EXPANSION {
        if s.contains(bad) {
            return (
                false,
                format!("forbidden shell expansion character {bad:?}"),
            );
        }
    }
    let Some(unquoted) = strip_quoted(s) else {
        return (false, "unbalanced quote".to_string());
    };
    let mut meta: Vec<char> = OPERATORS
        .iter()
        .copied()
        .filter(|c| unquoted.contains(*c))
        .collect();
    if !meta.is_empty() {
        meta.sort_unstable();
        let joined: String = meta.into_iter().collect();
        return (false, format!("unquoted shell metacharacter(s): {joined}"));
    }
    // Matches Steam's literal-substring substitution.
    if s.matches("%command%").count() != 1 {
        return (false, "must contain exactly one %command%".to_string());
    }
    // Safe now: balanced quotes, no unquoted operators.
    let Some(tokens) = shlex::split(s) else {
        return (false, "unparseable launch string".to_string());
    };
    if tokens.iter().filter(|token| *token == "%command%").count() != 1 {
        return (false, "%command% must be a standalone token".to_string());
    }
    for token in &tokens {
        if token == "%command%" {
            // No unquoted operators remain, so later tokens are game args.
            break;
        }
        if is_env_assign(token) {
            continue;
        }
        if token.starts_with('-') || token.starts_with('+') {
            // An option flag to a wrapper: inert; cannot name a program.
            continue;
        }
        if KNOWN_WRAPPERS.contains(&token.as_str()) {
            continue;
        }
        return (false, format!("unrecognized executable token {token:?}"));
    }
    (true, String::new())
}

// ------------------------------------------------------------------ fetching

pub trait Fetcher {
    fn get(&self, url: &str) -> Result<String, String>;
}

pub struct RealFetcher;

impl Fetcher for RealFetcher {
    fn get(&self, url: &str) -> Result<String, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(FETCH_TIMEOUT))
            .build()
            .into();
        // Fixed host, GET only.
        let mut response = agent.get(url).call().map_err(|err| err.to_string())?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|err| err.to_string())
    }
}

/// ProtonDB summary for appid, or None if unavailable.
///
/// Swallowing every error is deliberate: the advisor must degrade to "no
/// community data" on any network or parse failure, never crash.
pub fn protondb_summary(appid: &str, fetch: &dyn Fetcher) -> Option<Value> {
    let url = PROTONDB_URL.replace("{appid}", appid);
    let body = fetch.get(&url).ok()?;
    serde_json::from_str(&body).ok()
}

// ------------------------------------------------------------------ prompting

/// Python's `f"{value}"` for the JSON types a ProtonDB summary can hold. The
/// prompt was written against Python's rendering, so `True` and `None` rather
/// than `true` and `null`.
fn py_display(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(other) => other.to_string(),
    }
}

fn py_bool(flag: bool) -> &'static str {
    if flag {
        "True"
    } else {
        "False"
    }
}

pub fn build_prompt(
    game: &Game,
    profile: &SystemProfile,
    baseline: &str,
    protondb: Option<&Value>,
) -> String {
    let facts = [
        format!(
            "- GPU: {} (vendor={}, driver={})",
            profile.gpu_name, profile.gpu_vendor, profile.gpu_driver
        ),
        format!(
            "- Session: {} on {}, {}",
            profile.session, profile.desktop, profile.distro
        ),
        format!("- Runtime for this game: {}", game.runtime.as_str()),
        format!(
            "- Helpers present: gamemode={}, mangohud={}, gamescope={}",
            py_bool(profile.has_gamemode),
            py_bool(profile.has_mangohud),
            py_bool(profile.has_gamescope)
        ),
    ];

    let summary = match protondb {
        Some(Value::Object(map)) if !map.is_empty() => format!(
            "ProtonDB summary: tier={}, confidence={}, trendingTier={}, reports={}.",
            py_display(map.get("tier")),
            py_display(map.get("confidence")),
            py_display(map.get("trendingTier")),
            py_display(map.get("total"))
        ),
        _ => "ProtonDB summary: unavailable.".to_string(),
    };

    format!(
        "You are an expert Linux Steam gaming advisor. Recommend launch options \
         for the game \"{name}\" (appid {appid}) tuned to THIS machine.\n\n\
         This machine:\n{facts}\n\n\
         {summary}\n\n\
         The tool's generated hardware baseline for this game is:\n  {baseline}\n\
         In your answer, the literal token {{auto}} expands to exactly that \
         baseline. Prefer returning {{auto}} plus any game-specific tokens \
         (e.g. \"{{auto}} -dx11\") so the hardware baseline stays owned by the \
         tool.\n\n\
         Rules:\n\
         - Only suggest options that help THIS hardware/session; never anything \
         known to break the game. Be conservative.\n\
         - Allowed: KEY=VALUE env vars, known wrappers (gamemoderun, mangohud, \
         gamescope), and Steam launch flags. Exactly one %command%. No shell \
         metacharacters, no command substitution.\n\
         - If the baseline is already appropriate, return override=null.\n\n\
         Respond with STRICT JSON only, no prose outside it:\n\
         {{\"override\": string-or-null, \"reasoning\": string, \
         \"confidence\": \"low\"|\"medium\"|\"high\"}}",
        name = game.name,
        appid = game.appid,
        facts = facts.join("\n"),
        summary = summary,
        baseline = baseline,
    )
}

// ------------------------------------------------------------------ invoking

fn extract_json(text: &str) -> Result<Value, AdvisorError> {
    let start = text.find('{');
    let end = text.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(AdvisorError(format!(
            "no JSON object in LLM output: {:?}",
            truncate(text, 200)
        )));
    };
    if end < start {
        return Err(AdvisorError(format!(
            "no JSON object in LLM output: {:?}",
            truncate(text, 200)
        )));
    }
    serde_json::from_str(&text[start..=end])
        .map_err(|err| AdvisorError(format!("invalid JSON from LLM: {err}")))
}

fn truncate(text: &str, limit: usize) -> &str {
    match text.char_indices().nth(limit) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

pub fn run_llm(
    prompt: &str,
    command: &str,
    runner: &dyn CommandRunner,
) -> Result<Value, AdvisorError> {
    let Some(argv) = shlex::split(command) else {
        return Err(AdvisorError(
            "advisor_command is not a valid command".to_string(),
        ));
    };
    if argv.is_empty() {
        return Err(AdvisorError("advisor_command is empty".to_string()));
    }

    let output = runner
        .run(&argv, Some(prompt), LLM_TIMEOUT)
        .map_err(|err| match err {
            RunError::NotFound(program) => {
                AdvisorError(format!("advisor command not found: {program}"))
            }
            RunError::Timeout => AdvisorError("advisor command timed out".to_string()),
            RunError::Io(inner) => AdvisorError(format!("advisor command failed: {inner}")),
        })?;

    if !output.succeeded() {
        let status = output
            .status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "on a signal".to_string());
        return Err(AdvisorError(format!(
            "advisor command exited {status}: {}",
            truncate(output.stderr.trim(), 300)
        )));
    }

    let mut data = extract_json(&output.stdout)?;
    let Some(object) = data.as_object_mut() else {
        return Err(AdvisorError("LLM JSON is not an object".to_string()));
    };
    if !object.contains_key("override") {
        return Err(AdvisorError(
            "LLM JSON missing 'override' field".to_string(),
        ));
    }
    object
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::from(""));
    object
        .entry("confidence".to_string())
        .or_insert_with(|| Value::from("low"));
    Ok(data)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub appid: String,
    pub name: String,
    pub baseline: String,
    /// None means the LLM judged the baseline already appropriate.
    pub proposed: Option<String>,
    pub reasoning: String,
    pub confidence: String,
    pub valid: bool,
    pub warning: String,
}

pub fn advise(
    game: &Game,
    profile: &SystemProfile,
    config: &Config,
    fetch: &dyn Fetcher,
    runner: &dyn CommandRunner,
) -> Result<Proposal, AdvisorError> {
    let base = rules::baseline(game, profile, config);
    let summary = protondb_summary(&game.appid, fetch);
    let prompt = build_prompt(game, profile, &base, summary.as_ref());
    let data = run_llm(&prompt, &config.advisor_command(), runner)?;

    let raw = data.get("override");
    let proposed = match raw {
        None | Some(Value::Null) => None,
        Some(value) => Some(py_display(Some(value))),
    };
    let (valid, warning) = match &proposed {
        None => (true, String::new()),
        Some(value) => validate_override(&value.replace("{auto}", &base)),
    };

    Ok(Proposal {
        appid: game.appid.clone(),
        name: game.name.clone(),
        baseline: base,
        proposed,
        reasoning: py_display(data.get("reasoning")),
        confidence: py_display(data.get("confidence")),
        valid,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_assignment_keys_must_be_ascii_identifiers() {
        assert!(is_env_assign("FOO=1"));
        assert!(is_env_assign("_x9=1"));
        assert!(is_env_assign("FOO="));
        assert!(!is_env_assign("9FOO=1"));
        assert!(!is_env_assign("=1"));
        assert!(!is_env_assign("café=1"));
        assert!(!is_env_assign("no-equals"));
    }

    #[test]
    fn strip_quoted_removes_spans_and_detects_imbalance() {
        assert_eq!(strip_quoted("a\"b;c\"d").as_deref(), Some("ad"));
        assert_eq!(strip_quoted("a'b;c'd").as_deref(), Some("ad"));
        assert_eq!(strip_quoted("a\"bcd"), None);
    }

    #[test]
    fn truncate_does_not_split_a_character() {
        assert_eq!(truncate("héllo", 3), "hél");
        assert_eq!(truncate("hi", 100), "hi");
    }
}
