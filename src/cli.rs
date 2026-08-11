//! Command-line interface: scan / apply / status / revert / setup / doctor.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value};

use crate::apply::{self, ApplyError, Change, State};
use crate::codes::{Action, Guardrail, Outcome};
use crate::doctor::{self, Finding};
use crate::jsonio::{Emitter, Fields, Kind};
use crate::proc::{self, CommandRunner};
use crate::rules::{self, Config, ConfigError};
use crate::steam::{self, Game};
use crate::sysinfo::{self, SystemProbe, SystemProfile};

const VENDORS: [&str; 3] = ["nvidia", "amd", "intel"];
const PROGRESS_EVERY: usize = 50;

/// What can stop a command before it produces output.
///
/// The two are kept apart because only one of them has a wire code: a bad
/// config is `config-invalid`, which a client can act on, while an unreadable
/// state file is a plain error with a message to read.
enum CliError {
    Config(ConfigError),
    Apply(ApplyError),
}

impl CliError {
    fn guardrail(&self) -> Option<&'static str> {
        match self {
            CliError::Config(_) => Some(Guardrail::ConfigInvalid.as_str()),
            CliError::Apply(_) => None,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Config(err) => write!(f, "{err}"),
            CliError::Apply(err) => write!(f, "{err}"),
        }
    }
}

impl From<ConfigError> for CliError {
    fn from(err: ConfigError) -> Self {
        CliError::Config(err)
    }
}

impl From<ApplyError> for CliError {
    fn from(err: ApplyError) -> Self {
        CliError::Apply(err)
    }
}

fn vendor_display_name(vendor: &str) -> &'static str {
    match vendor {
        "nvidia" => "NVIDIA GPU",
        "amd" => "AMD GPU",
        _ => "Intel GPU",
    }
}

// ------------------------------------------------------------------- parser

#[derive(Parser, Debug)]
#[command(
    name = "steamtrain",
    version,
    about = "Set hardware-appropriate Steam launch options for installed games.",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// show installed games and proposed launch options
    Scan(CommonArgs),
    /// write launch options (skips anything a human set)
    Apply(ApplyArgs),
    /// show what this tool manages and last state
    Status(CommonArgs),
    /// restore every option this tool set back to empty
    Revert(CommonArgs),
    /// confirm detected hardware; pick or clear the GPU vendor if wrong
    Setup(SetupArgs),
    /// diagnose install problems; --fix removes an old ~/.local install
    Doctor(DoctorArgs),
}

#[derive(Args, Debug, Clone)]
struct Locations {
    /// Steam root (auto-detected)
    #[arg(long, value_name = "PATH")]
    steam_root: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    state_dir: Option<PathBuf>,
}

impl Locations {
    fn config_path(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(rules::default_config_path)
    }

    fn state_path(&self) -> PathBuf {
        self.state_dir
            .clone()
            .unwrap_or_else(apply::default_state_dir)
    }
}

#[derive(Args, Debug, Clone)]
struct CommonArgs {
    #[command(flatten)]
    locations: Locations,
    /// machine-readable newline-delimited JSON on stdout
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct ApplyArgs {
    #[command(flatten)]
    common: CommonArgs,
    /// plan only, write nothing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug, Clone)]
struct SetupArgs {
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// set the GPU vendor and exit without prompting ('auto' clears the
    /// override and restores autodetection)
    #[arg(long, value_parser = ["nvidia", "amd", "intel", "auto"])]
    gpu_vendor: Option<String>,
    /// machine-readable newline-delimited JSON on stdout
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug, Clone)]
struct DoctorArgs {
    /// repair what can be repaired
    #[arg(long)]
    fix: bool,
    /// with --fix, list what would be removed and remove nothing
    #[arg(long)]
    dry_run: bool,
    /// treat an old ~/.local install as a conflict even when no packaged
    /// install is present (used by install.sh --migrate)
    #[arg(long)]
    force: bool,
    /// machine-readable newline-delimited JSON on stdout
    #[arg(long)]
    json: bool,
}

// --------------------------------------------------------------- injection

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub input: &'a mut dyn BufRead,
}

/// Everything the CLI reaches the outside world through. Tests substitute all
/// of it; `Deps::default()` wires the real implementations.
pub struct Deps<'a> {
    pub probe: &'a dyn SystemProbe,
    pub is_running: &'a dyn Fn(&Path) -> bool,
    pub runner: &'a dyn CommandRunner,
    pub doctor: doctor::Options,
}

static REAL_PROBE: sysinfo::RealProbe = sysinfo::RealProbe;
static REAL_RUNNER: proc::RealRunner = proc::RealRunner;
static REAL_IS_RUNNING: fn(&Path) -> bool = steam::is_steam_running;

impl Default for Deps<'static> {
    fn default() -> Self {
        Deps {
            probe: &REAL_PROBE,
            is_running: &REAL_IS_RUNNING,
            runner: &REAL_RUNNER,
            doctor: doctor::Options::default(),
        }
    }
}

// ----------------------------------------------------------------- helpers

/// Steam root from the flag or autodetection. Reports its own failure, since
/// the shape of that report differs between the two output modes.
fn context(steam_root: Option<&Path>, out: &mut Emitter, err: &mut dyn Write) -> Option<PathBuf> {
    if let Some(root) = steam_root {
        return Some(root.to_path_buf());
    }
    match steam::find_steam_root() {
        Some(root) => Some(root),
        None => {
            if out.enabled() {
                out.result(
                    false,
                    Outcome::Error,
                    Fields::new()
                        .set("guardrail", Guardrail::NoSteamRoot.as_str())
                        .set("message", "no Steam installation found"),
                );
            } else {
                let _ = writeln!(err, "ERROR: no Steam installation found");
            }
            None
        }
    }
}

/// Detected system profile with the config's gpu_vendor override applied.
///
/// An empty override means autodetect (unchanged behaviour); an unrecognized
/// value is ignored with a warning and autodetection is used. Matching is
/// case-insensitive so the on-screen labels (NVIDIA/AMD/Intel) also work.
fn profile_for(config: &Config, deps: &Deps, err: &mut dyn Write) -> SystemProfile {
    let mut profile = sysinfo::detect(deps.probe);
    let raw = config.gpu_vendor_raw();
    let vendor = raw.trim().to_lowercase();
    if vendor.is_empty() {
        return profile;
    }
    if VENDORS.contains(&vendor.as_str()) {
        if profile.gpu_name.is_empty() {
            profile.gpu_name = vendor_display_name(&vendor).to_string();
        }
        if profile.gpu_driver.is_empty() {
            profile.gpu_driver = "set via steamtrain setup".to_string();
        }
        profile.gpu_vendor = vendor;
        return profile;
    }
    let _ = writeln!(
        err,
        "WARNING: ignoring unrecognized gpu_vendor {}; using autodetection",
        crate::py_repr(&raw)
    );
    profile
}

struct Proposals {
    profile: SystemProfile,
    games: Vec<Game>,
    options: BTreeMap<String, String>,
    names: BTreeMap<String, String>,
    excluded: Vec<String>,
}

fn proposals(
    root: &Path,
    config_path: &Path,
    deps: &Deps,
    err: &mut dyn Write,
) -> Result<Proposals, ConfigError> {
    let config = rules::load_config(config_path)?;
    let profile = profile_for(&config, deps, err);
    let games = steam::installed_games(root);
    let mut options = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut excluded = Vec::new();
    for game in &games {
        match rules::build_options(game, &profile, &config) {
            Some(value) => {
                options.insert(game.appid.clone(), value);
                names.insert(game.appid.clone(), game.name.clone());
            }
            None => excluded.push(game.appid.clone()),
        }
    }
    Ok(Proposals {
        profile,
        games,
        options,
        names,
        excluded,
    })
}

fn emit_profile(out: &mut Emitter, profile: &SystemProfile) {
    let value = serde_json::to_value(profile).unwrap_or(Value::Null);
    out.emit(Kind::Profile, Fields::new().merge(value));
}

/// Display metadata only; state lives on change records.
fn emit_games(out: &mut Emitter, games: &[Game]) {
    for game in games {
        out.emit(
            Kind::Game,
            Fields::new()
                .set("appid", game.appid.as_str())
                .set("name", game.name.as_str())
                .set("runtime", game.runtime.as_str())
                .set("library", game.library.display().to_string()),
        );
    }
}

/// One record per (user, appid) so exclusions share change identity.
///
/// The planner never sees these - build_options drops excluded games before
/// planning - so they are re-introduced here, which is what lets a client show
/// that an exclusion is being honoured rather than losing the game.
fn excluded_records(root: &Path, excluded: &[String]) -> Vec<(String, String)> {
    let mut records = Vec::new();
    for (user, _) in steam::user_localconfigs(root) {
        for appid in excluded {
            records.push((user.clone(), appid.clone()));
        }
    }
    records
}

/// Emit every change record with progress, and return counts by action.
fn emit_changes(
    out: &mut Emitter,
    changes: &[Change],
    excluded: &[(String, String)],
) -> Map<String, Value> {
    let total = changes.len() + excluded.len();
    let mut counts: BTreeMap<&str, u64> = Action::ALL
        .iter()
        .map(|action| (action.as_str(), 0))
        .collect();
    let mut index = 0usize;

    let tick = |out: &mut Emitter, index: usize| {
        if index % PROGRESS_EVERY == 0 || index == total {
            out.emit(
                Kind::Progress,
                Fields::new()
                    .set("done", index as u64)
                    .set("total", total as u64),
            );
        }
    };

    for change in changes {
        out.emit(
            Kind::Change,
            Fields::new()
                .set("user", change.user.as_str())
                .set("appid", change.appid.as_str())
                .set("current", change.current.as_str())
                .set("proposed", change.proposed.as_str())
                .set("action", change.action.as_str()),
        );
        *counts.entry(change.action.as_str()).or_insert(0) += 1;
        index += 1;
        tick(out, index);
    }
    for (user, appid) in excluded {
        out.emit(
            Kind::Change,
            Fields::new()
                .set("user", user.as_str())
                .set("appid", appid.as_str())
                .set("action", Action::Excluded.as_str()),
        );
        *counts.entry(Action::Excluded.as_str()).or_insert(0) += 1;
        index += 1;
        tick(out, index);
    }

    // Keyed in Action::ALL order, and always with every action present so a
    // client reads "none of these happened" rather than "the key is missing".
    let mut ordered = Map::new();
    for action in Action::ALL {
        ordered.insert(
            action.as_str().to_string(),
            Value::from(counts.get(action.as_str()).copied().unwrap_or(0)),
        );
    }
    ordered
}

fn marker(action: Action) -> &'static str {
    match action {
        Action::Set => "SET ",
        Action::SkipUnchanged => "ok  ",
        Action::SkipUserSet => "KEEP",
        // Never reached: excluded records exist only in the JSON stream.
        Action::Excluded => "excl",
    }
}

fn or_empty(value: &str) -> &str {
    if value.is_empty() {
        "(empty)"
    } else {
        value
    }
}

fn print_changes(out: &mut Emitter, changes: &[Change]) {
    for change in changes {
        let _ = writeln!(
            out.writer(),
            "  [{}] user {}  {:>8}  {}",
            marker(change.action),
            change.user,
            change.appid,
            change.name
        );
        match change.action {
            Action::Set => {
                let _ = writeln!(
                    out.writer(),
                    "           {}  ->  {}",
                    or_empty(&change.current),
                    or_empty(&change.proposed)
                );
            }
            Action::SkipUserSet => {
                let _ = writeln!(
                    out.writer(),
                    "           keeping human-set value: {}",
                    change.current
                );
            }
            _ => {}
        }
    }
}

fn yes_no(flag: bool) -> &'static str {
    if flag {
        "yes"
    } else {
        "no"
    }
}

// ------------------------------------------------------------------ scan

fn cmd_scan(args: &CommonArgs, io: &mut Io, deps: &Deps) -> Result<i32, CliError> {
    let mut out = Emitter::new(&mut *io.out, args.json);
    let Some(root) = context(args.locations.steam_root.as_deref(), &mut out, &mut *io.err) else {
        return Ok(1);
    };
    let plan = proposals(&root, &args.locations.config_path(), deps, &mut *io.err)?;

    if out.enabled() {
        let state = State::load(&args.locations.state_path())?;
        let changes = apply::plan_changes(&root, &plan.options, &state, &plan.names)?;
        emit_profile(&mut out, &plan.profile);
        emit_games(&mut out, &plan.games);
        let counts = emit_changes(&mut out, &changes, &excluded_records(&root, &plan.excluded));
        out.result(
            true,
            Outcome::Ok,
            Fields::new()
                .set("counts", Value::Object(counts))
                .set("steam_running", (deps.is_running)(&root)),
        );
        return Ok(0);
    }

    let profile = &plan.profile;
    let _ = writeln!(
        out.writer(),
        "System: {} | {}/{} | {} ({} {}) | gamemode={} mangohud={}",
        profile.distro,
        profile.desktop,
        profile.session,
        profile.gpu_name,
        profile.gpu_vendor,
        profile.gpu_driver,
        yes_no(profile.has_gamemode),
        yes_no(profile.has_mangohud)
    );
    let _ = writeln!(
        out.writer(),
        "Steam root: {}  (running: {})",
        root.display(),
        yes_no((deps.is_running)(&root))
    );
    if plan.games.is_empty() {
        let _ = writeln!(
            out.writer(),
            "No installed games found on mounted libraries."
        );
        return Ok(0);
    }
    let _ = writeln!(
        out.writer(),
        "\n{} installed game(s) on disk:",
        plan.games.len()
    );
    for game in &plan.games {
        let _ = writeln!(
            out.writer(),
            "  {:>8}  {:<7}  {}",
            game.appid,
            game.runtime.as_str(),
            game.name
        );
        let _ = writeln!(
            out.writer(),
            "           library: {}",
            game.library.display()
        );
        let _ = writeln!(
            out.writer(),
            "           proposed: {}",
            plan.options
                .get(&game.appid)
                .map(String::as_str)
                .unwrap_or("(excluded)")
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------- status

fn cmd_status(args: &CommonArgs, io: &mut Io, deps: &Deps) -> Result<i32, CliError> {
    let mut out = Emitter::new(&mut *io.out, args.json);
    let Some(root) = context(args.locations.steam_root.as_deref(), &mut out, &mut *io.err) else {
        return Ok(1);
    };
    let state = State::load(&args.locations.state_path())?;

    if out.enabled() {
        // config_exists is probed directly and never through load_config,
        // which creates the file as a side effect of reading it. Reading
        // status must not be what makes the first-run screen stop appearing.
        out.result(
            true,
            Outcome::Ok,
            Fields::new()
                .set("config_exists", args.locations.config_path().is_file())
                .set("managed", Value::Object(state.data().clone()))
                .set("steam_running", (deps.is_running)(&root)),
        );
        return Ok(0);
    }

    if state.is_empty() {
        let _ = writeln!(
            out.writer(),
            "No launch options are currently managed by this tool."
        );
        return Ok(0);
    }
    let _ = writeln!(out.writer(), "{} managed launch option(s):", state.len());
    let mut entries: Vec<(String, String)> = state
        .data()
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    entries.sort();
    for (key, value) in entries {
        let _ = writeln!(out.writer(), "  {key}: {value}");
    }
    Ok(0)
}

// ----------------------------------------------------------------- apply

/// Execute planned changes and close the stream with the run outcome.
///
/// A guardrail refusal exits 0, not non-zero: Steam being open is the expected
/// case, and the timer must not record a failure for it.
fn write_json(
    out: &mut Emitter,
    root: &Path,
    changes: &[Change],
    state_dir: &Path,
    counts: Map<String, Value>,
    deps: &Deps,
) -> i32 {
    match apply::apply_changes(root, changes, state_dir, deps.is_running) {
        Ok(written) => {
            out.result(
                true,
                Outcome::Ok,
                Fields::new()
                    .set("counts", Value::Object(counts))
                    .set("written", written.len() as u64),
            );
            0
        }
        Err(ApplyError::SteamRunning(message)) => {
            out.result(
                false,
                Outcome::Blocked,
                Fields::new()
                    .set("guardrail", Guardrail::SteamRunning.as_str())
                    .set("message", message.as_str())
                    .set("counts", Value::Object(counts))
                    .set("written", 0),
            );
            0
        }
        Err(other) => {
            out.result(
                false,
                Outcome::Error,
                Fields::new()
                    .set("message", other.to_string())
                    .set("counts", Value::Object(counts))
                    .set("written", 0),
            );
            1
        }
    }
}

fn cmd_apply(args: &ApplyArgs, io: &mut Io, deps: &Deps) -> Result<i32, CliError> {
    let common = &args.common;
    let mut out = Emitter::new(&mut *io.out, common.json);
    let Some(root) = context(
        common.locations.steam_root.as_deref(),
        &mut out,
        &mut *io.err,
    ) else {
        return Ok(1);
    };
    let plan = proposals(&root, &common.locations.config_path(), deps, &mut *io.err)?;
    let state_dir = common.locations.state_path();
    let state = State::load(&state_dir)?;
    let changes = apply::plan_changes(&root, &plan.options, &state, &plan.names)?;

    if out.enabled() {
        emit_profile(&mut out, &plan.profile);
        emit_games(&mut out, &plan.games);
        let counts = emit_changes(&mut out, &changes, &excluded_records(&root, &plan.excluded));
        if args.dry_run {
            out.result(
                true,
                Outcome::Ok,
                Fields::new()
                    .set("counts", Value::Object(counts))
                    .set("dry_run", true)
                    .set("steam_running", (deps.is_running)(&root)),
            );
            return Ok(0);
        }
        return Ok(write_json(
            &mut out, &root, &changes, &state_dir, counts, deps,
        ));
    }

    print_changes(&mut out, &changes);
    if args.dry_run {
        let planned = changes
            .iter()
            .filter(|change| change.action == Action::Set)
            .count();
        let _ = writeln!(
            out.writer(),
            "dry-run: {planned} change(s) would be written, nothing touched"
        );
        return Ok(0);
    }
    match apply::apply_changes(&root, &changes, &state_dir, deps.is_running) {
        Ok(written) => {
            let _ = writeln!(
                out.writer(),
                "{} set, {} skipped",
                written.len(),
                changes.len() - written.len()
            );
            Ok(0)
        }
        // Expected condition; the timer retries later.
        Err(ApplyError::SteamRunning(message)) => {
            let _ = writeln!(out.writer(), "NOTE: {message}");
            Ok(0)
        }
        Err(other) => {
            let _ = writeln!(io.err, "ERROR: {other}");
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------- revert

fn cmd_revert(args: &CommonArgs, io: &mut Io, deps: &Deps) -> Result<i32, CliError> {
    let mut out = Emitter::new(&mut *io.out, args.json);
    let Some(root) = context(args.locations.steam_root.as_deref(), &mut out, &mut *io.err) else {
        return Ok(1);
    };
    let state_dir = args.locations.state_path();
    let state = State::load(&state_dir)?;
    let changes = apply::plan_revert(&root, &state)?;

    if out.enabled() {
        // No game records here: revert acts on state, which can hold appids
        // that are no longer installed. Clients render those by appid.
        let counts = emit_changes(&mut out, &changes, &[]);
        return Ok(write_json(
            &mut out, &root, &changes, &state_dir, counts, deps,
        ));
    }

    if changes.is_empty() {
        let _ = writeln!(out.writer(), "Nothing to revert.");
        return Ok(0);
    }
    print_changes(&mut out, &changes);
    match apply::apply_changes(&root, &changes, &state_dir, deps.is_running) {
        Ok(written) => {
            let _ = writeln!(out.writer(), "{} reverted", written.len());
            Ok(0)
        }
        Err(ApplyError::SteamRunning(message)) => {
            let _ = writeln!(out.writer(), "NOTE: {message}");
            Ok(0)
        }
        Err(other) => {
            let _ = writeln!(io.err, "ERROR: {other}");
            Ok(1)
        }
    }
}

// ----------------------------------------------------------------- setup

/// Menu "Skip" is distinct from "" (clear the override, restoring
/// autodetection): one changes nothing, the other changes the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VendorChoice {
    Vendor(&'static str),
    Autodetect,
    Skip,
}

const VENDOR_MENU: [(&str, VendorChoice, &str); 5] = [
    ("1", VendorChoice::Vendor("nvidia"), "NVIDIA"),
    ("2", VendorChoice::Vendor("amd"), "AMD"),
    ("3", VendorChoice::Vendor("intel"), "Intel"),
    ("4", VendorChoice::Autodetect, "Autodetect (clear override)"),
    ("5", VendorChoice::Skip, "Skip (no change)"),
];

fn read_line(input: &mut dyn BufRead) -> Option<String> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Numbered menu -> the chosen action, or None on end of input.
fn prompt_gpu_vendor(out: &mut Emitter, input: &mut dyn BufRead) -> Option<VendorChoice> {
    for (number, _, label) in VENDOR_MENU {
        let _ = writeln!(out.writer(), "  {number}) {label}");
    }
    loop {
        let _ = write!(
            out.writer(),
            "Select your GPU vendor [1-{}]: ",
            VENDOR_MENU.len()
        );
        let _ = out.writer().flush();
        let Some(raw) = read_line(input) else {
            let _ = writeln!(out.writer());
            return None;
        };
        let raw = raw.trim();
        if let Some((_, choice, _)) = VENDOR_MENU.iter().find(|(number, _, _)| *number == raw) {
            return Some(*choice);
        }
        let _ = writeln!(
            out.writer(),
            "Please enter a number 1-{}.",
            VENDOR_MENU.len()
        );
    }
}

/// [Y/n] confirm; empty input or end of input counts as yes.
fn confirm(prompt: &str, out: &mut Emitter, input: &mut dyn BufRead) -> bool {
    loop {
        let _ = write!(out.writer(), "{prompt}");
        let _ = out.writer().flush();
        let Some(raw) = read_line(input) else {
            let _ = writeln!(out.writer());
            return true;
        };
        match raw.trim().to_lowercase().as_str() {
            "" | "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => {
                let _ = writeln!(out.writer(), "Please answer y or n.");
            }
        }
    }
}

/// Set the GPU vendor without prompting, for the desktop interface.
///
/// The interactive wizard cannot be driven by a GUI, and the GUI must not
/// write config.json itself - every write goes through the Core. 'auto' clears
/// the override rather than storing a literal, so autodetection resumes.
fn setup_noninteractive(args: &SetupArgs, io: &mut Io) -> i32 {
    let mut out = Emitter::new(&mut *io.out, args.json);
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(rules::default_config_path);
    let requested = args.gpu_vendor.as_deref().unwrap_or("auto");
    let vendor = if requested == "auto" { "" } else { requested };

    if let Err(err) = rules::save_gpu_vendor(&config_path, vendor) {
        if out.enabled() {
            out.result(
                false,
                Outcome::Error,
                Fields::new().set(
                    "message",
                    format!("could not write {}: {err}", config_path.display()),
                ),
            );
        } else {
            let _ = writeln!(
                io.err,
                "ERROR: could not write {}: {err}",
                config_path.display()
            );
        }
        return 1;
    }

    if out.enabled() {
        out.result(
            true,
            Outcome::Ok,
            Fields::new()
                .set("gpu_vendor", vendor)
                .set("config_path", config_path.display().to_string()),
        );
    } else if vendor.is_empty() {
        let _ = writeln!(
            out.writer(),
            "Cleared gpu_vendor in {}; autodetection is back in effect.",
            config_path.display()
        );
    } else {
        let _ = writeln!(
            out.writer(),
            "Saved gpu_vendor={} to {}.",
            crate::py_repr(vendor),
            config_path.display()
        );
    }
    0
}

fn cmd_setup(args: &SetupArgs, io: &mut Io, deps: &Deps) -> Result<i32, CliError> {
    if args.gpu_vendor.is_some() {
        return Ok(setup_noninteractive(args, io));
    }
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(rules::default_config_path);
    let profile = sysinfo::detect(deps.probe);
    let config = rules::load_config(&config_path)?;
    let mut out = Emitter::new(&mut *io.out, false);

    let driver = if profile.gpu_driver.is_empty() {
        String::new()
    } else {
        format!(" {}", profile.gpu_driver)
    };
    let _ = writeln!(out.writer(), "Detected hardware profile:");
    let _ = writeln!(out.writer(), "  distro : {}", profile.distro);
    let _ = writeln!(
        out.writer(),
        "  desktop: {} ({})",
        profile.desktop,
        profile.session
    );
    let gpu_name = if profile.gpu_name.is_empty() {
        "unknown"
    } else {
        &profile.gpu_name
    };
    let _ = writeln!(
        out.writer(),
        "  GPU    : {gpu_name} [{}]{driver}",
        profile.gpu_vendor
    );
    let _ = writeln!(
        out.writer(),
        "  helpers: gamemode={} mangohud={}",
        yes_no(profile.has_gamemode),
        yes_no(profile.has_mangohud)
    );

    Ok(setup_interact(
        &config_path,
        &profile,
        &config.gpu_vendor_raw(),
        &mut out,
        io.input,
        io.err,
    ))
}

fn setup_interact(
    config_path: &Path,
    profile: &SystemProfile,
    override_value: &str,
    out: &mut Emitter,
    input: &mut dyn BufRead,
    err: &mut dyn Write,
) -> i32 {
    let clear_hint = "answer n, then pick 'Autodetect (clear override)' to remove it";
    let recognised = VENDORS.contains(&override_value);
    let quoted = crate::py_repr(override_value);

    if profile.gpu_vendor != "unknown" {
        if recognised {
            let _ = writeln!(
                out.writer(),
                "\nGPU autodetected as {}; config override gpu_vendor={quoted} is \
                 active and wins over autodetection ({clear_hint}).",
                profile.gpu_vendor
            );
        } else if !override_value.is_empty() {
            let _ = writeln!(
                out.writer(),
                "\nGPU autodetected as {}; config value gpu_vendor={quoted} is not \
                 recognized and is ignored ({clear_hint}).",
                profile.gpu_vendor
            );
        } else {
            let _ = writeln!(
                out.writer(),
                "\nGPU autodetected as {}; no override needed.",
                profile.gpu_vendor
            );
        }
        let effective = if recognised {
            override_value
        } else {
            &profile.gpu_vendor
        };
        if confirm(
            &format!("\nUse {effective} for launch options? [Y/n]: "),
            out,
            input,
        ) {
            let _ = writeln!(out.writer(), "Keeping {effective}. Nothing written.");
            return 0;
        }
        let _ = writeln!(
            out.writer(),
            "\nChange it — pick the GPU that drives your games:"
        );
    } else if recognised {
        let _ = writeln!(
            out.writer(),
            "\nGPU autodetection failed, but config override gpu_vendor={quoted} is \
             active — scan/apply already use it."
        );
        let _ = writeln!(
            out.writer(),
            "Pick a vendor to change it, or Skip to keep it:"
        );
    } else {
        if !override_value.is_empty() {
            let _ = writeln!(
                out.writer(),
                "\nNOTE: config value gpu_vendor={quoted} is not recognized and is ignored."
            );
        }
        let _ = writeln!(
            out.writer(),
            "\nGPU vendor could not be autodetected. Pick it so scan/apply set \
             vendor-appropriate options:"
        );
    }

    let choice = prompt_gpu_vendor(out, input);
    let chosen = match choice {
        None | Some(VendorChoice::Skip) => {
            if recognised {
                let _ = writeln!(
                    out.writer(),
                    "No change made; override gpu_vendor={quoted} stays in effect."
                );
            } else if !override_value.is_empty() {
                let _ = writeln!(
                    out.writer(),
                    "No change made; unrecognized gpu_vendor={quoted} stays in the config \
                     (ignored) and autodetection governs."
                );
            } else {
                let _ = writeln!(
                    out.writer(),
                    "No change made; GPU autodetection stays in effect."
                );
            }
            return 0;
        }
        Some(VendorChoice::Autodetect) => "",
        Some(VendorChoice::Vendor(vendor)) => vendor,
    };

    if chosen.is_empty() && override_value.is_empty() {
        let _ = writeln!(
            out.writer(),
            "No override set; GPU autodetection is already in effect. Nothing written."
        );
        return 0;
    }
    if let Err(error) = rules::save_gpu_vendor(config_path, chosen) {
        let _ = writeln!(
            err,
            "ERROR: could not write {}: {error}",
            config_path.display()
        );
        return 1;
    }
    if chosen.is_empty() {
        let _ = writeln!(
            out.writer(),
            "\nCleared gpu_vendor in {}; GPU autodetection is back in effect.",
            config_path.display()
        );
    } else {
        let _ = writeln!(
            out.writer(),
            "\nSaved gpu_vendor={} to {}.",
            crate::py_repr(chosen),
            config_path.display()
        );
    }
    let _ = writeln!(
        out.writer(),
        "Nothing is written to Steam yet — the next `steamtrain apply` (or timer run) \
         uses it; restart Steam afterwards to see the options in the UI."
    );
    0
}

// ---------------------------------------------------------------- doctor

fn emit_findings(out: &mut Emitter, findings: &[Finding], to_stderr: Option<&mut dyn Write>) {
    if out.enabled() {
        for finding in findings {
            out.emit(
                Kind::Finding,
                Fields::new()
                    .set("code", finding.code.as_str())
                    .set("message", finding.message.as_str())
                    .set("paths", finding.paths.clone())
                    .set("fixable", finding.fixable),
            );
        }
        return;
    }
    match to_stderr {
        Some(stream) => write_findings(stream, findings),
        None => write_findings(out.writer(), findings),
    }
}

fn write_findings(stream: &mut dyn Write, findings: &[Finding]) {
    for finding in findings {
        let _ = writeln!(stream, "PROBLEM: {}.", finding.message);
        for path in &finding.paths {
            let _ = writeln!(stream, "         {path}");
        }
    }
}

fn cmd_doctor(args: &DoctorArgs, io: &mut Io, deps: &Deps) -> i32 {
    let mut out = Emitter::new(&mut *io.out, args.json);
    let mut options = deps.doctor.clone();
    options.force = args.force;
    let findings = doctor::diagnose(&options);

    if findings.is_empty() {
        if out.enabled() {
            out.result(
                true,
                Outcome::Ok,
                Fields::new().set("findings", 0).set("fixed", 0),
            );
        } else {
            let _ = writeln!(out.writer(), "No problems found.");
        }
        return 0;
    }

    emit_findings(&mut out, &findings, None);
    if !args.fix {
        if out.enabled() {
            out.result(
                false,
                Outcome::Error,
                Fields::new()
                    .set("findings", findings.len() as u64)
                    .set("fixed", 0)
                    .set("message", "run `steamtrain doctor --fix` to repair"),
            );
        } else {
            let _ = writeln!(out.writer(), "\nRun `steamtrain doctor --fix` to repair.");
        }
        return 2;
    }

    let (removed, failed) = doctor::migrate(&options.home(), args.dry_run, deps.runner);
    if out.enabled() {
        let ok = failed.is_empty() && !args.dry_run;
        let removed_records: Vec<Value> = removed
            .iter()
            .map(|(path, detail)| serde_json::json!({ "path": path, "detail": detail }))
            .collect();
        let failed_records: Vec<Value> = failed
            .iter()
            .map(|(path, error)| serde_json::json!({ "path": path, "error": error }))
            .collect();
        out.result(
            ok,
            if ok { Outcome::Ok } else { Outcome::Error },
            Fields::new()
                .set("findings", findings.len() as u64)
                .set(
                    "fixed",
                    if args.dry_run {
                        0
                    } else {
                        removed.len() as u64
                    },
                )
                .set("removed", removed_records)
                .set("failed", failed_records)
                .set("dry_run", args.dry_run),
        );
    } else {
        for (path, detail) in &removed {
            let _ = writeln!(out.writer(), "  {detail}: {path}");
        }
        for (path, error) in &failed {
            let _ = writeln!(io.err, "  FAILED: {path}: {error}");
        }
        if args.dry_run {
            let _ = writeln!(out.writer(), "\ndry-run: nothing was removed");
        } else if !failed.is_empty() {
            let _ = writeln!(
                io.err,
                "\nMigration incomplete; the paths above still need removing."
            );
        } else {
            let _ = writeln!(
                out.writer(),
                "\nMigrated. Configuration and state were left untouched."
            );
        }
    }
    if !failed.is_empty() || args.dry_run {
        2
    } else {
        0
    }
}

// ------------------------------------------------------------------ main

fn json_enabled(command: &Command) -> bool {
    match command {
        Command::Scan(args) | Command::Status(args) | Command::Revert(args) => args.json,
        Command::Apply(args) => args.common.json,
        Command::Setup(args) => args.json,
        Command::Doctor(args) => args.json,
    }
}

/// A shadowed install must never run silently.
///
/// Cheap on a healthy machine - diagnose() returns immediately when no
/// packaged install exists, which is every developer checkout.
fn warn_legacy(command: &Command, io: &mut Io, deps: &Deps) {
    let findings = doctor::diagnose(&deps.doctor);
    if findings.is_empty() {
        return;
    }
    let json = json_enabled(command);
    let mut out = Emitter::new(&mut *io.out, json);
    if json {
        emit_findings(&mut out, &findings, None);
        return;
    }
    emit_findings(&mut out, &findings, Some(&mut *io.err));
    let _ = writeln!(
        io.err,
        "         The packaged install is not the code being run."
    );
    let _ = writeln!(
        io.err,
        "         Fix automatically: steamtrain doctor --fix"
    );
}

pub fn main(argv: &[String], io: &mut Io, deps: &Deps) -> i32 {
    let full = std::iter::once("steamtrain".to_string()).chain(argv.iter().cloned());
    let parsed = match Cli::try_parse_from(full) {
        Ok(parsed) => parsed,
        Err(err) => {
            // clap sends --help and --version to stdout with status 0, and
            // usage errors to stderr with status 2 - the same split argparse
            // used.
            let target: &mut dyn Write = if err.use_stderr() {
                &mut *io.err
            } else {
                &mut *io.out
            };
            let _ = write!(target, "{err}");
            return err.exit_code();
        }
    };

    if !matches!(parsed.command, Command::Doctor(_)) {
        warn_legacy(&parsed.command, io, deps);
    }

    let outcome = match &parsed.command {
        Command::Scan(args) => cmd_scan(args, io, deps),
        Command::Apply(args) => cmd_apply(args, io, deps),
        Command::Status(args) => cmd_status(args, io, deps),
        Command::Revert(args) => cmd_revert(args, io, deps),
        Command::Setup(args) => cmd_setup(args, io, deps),
        Command::Doctor(args) => return cmd_doctor(args, io, deps),
    };

    match outcome {
        Ok(code) => code,
        Err(err) => {
            let json = json_enabled(&parsed.command);
            let mut out = Emitter::new(&mut *io.out, json);
            if json {
                let mut fields = Fields::new();
                if let Some(guardrail) = err.guardrail() {
                    fields = fields.set("guardrail", guardrail);
                }
                out.result(
                    false,
                    Outcome::Error,
                    fields.set("message", err.to_string()),
                );
            } else {
                let _ = writeln!(io.err, "ERROR: {err}");
            }
            1
        }
    }
}
