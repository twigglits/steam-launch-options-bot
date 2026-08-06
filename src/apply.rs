//! Safe writer for LaunchOptions in each user's localconfig.vdf.
//!
//! Safety contract:
//! - Refuses to write while the owning Steam client runs (Steam rewrites
//!   localconfig.vdf on exit, silently discarding edits made underneath it).
//! - Never clobbers options a human set: writes only when the current value is
//!   empty or byte-equal to what this tool wrote before (tracked in a state
//!   file), so manual tweaks always win.
//! - Timestamped backup of every file before modification, newest 10 kept.
//! - Atomic replace, permissions preserved.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::codes::Action;
use crate::steam;
use crate::vdf;

pub const BACKUPS_PER_USER: usize = 10;

const APPS_PATH: [&[u8]; 5] = [
    b"UserLocalConfigStore",
    b"Software",
    b"Valve",
    b"Steam",
    b"apps",
];

pub fn default_state_dir() -> PathBuf {
    crate::home().join(".local/state/steamtrain")
}

#[derive(Debug)]
pub enum ApplyError {
    /// The guardrail, not a failure: exits 0 and the timer retries later.
    SteamRunning(String),
    Io(String),
    Vdf(String),
    /// The state file exists but cannot be read. Deliberately fatal - see
    /// `State::load`.
    State(String),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::SteamRunning(message)
            | ApplyError::Io(message)
            | ApplyError::Vdf(message)
            | ApplyError::State(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ApplyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub user: String,
    pub appid: String,
    pub name: String,
    pub current: String,
    pub proposed: String,
    pub action: Action,
}

/// Remembers what we wrote, per "user/appid", so we only ever update values
/// that are still our own.
#[derive(Debug, Clone, Default)]
pub struct State {
    data: Map<String, Value>,
}

impl State {
    /// Load the state file. A missing file is an empty state - that is a first
    /// run.
    ///
    /// A file that exists but cannot be parsed is an error rather than an
    /// empty state, and that is deliberate. Treating it as empty would leave
    /// the tool quietly unable to recognise the options it had already
    /// written: it would never overwrite them (they would read as
    /// human-set, which is safe), but `revert` would no longer clear them, and
    /// the user would have no way to tell. Failing loudly is recoverable;
    /// silently forgetting is not.
    pub fn load(state_dir: &Path) -> Result<State, ApplyError> {
        let path = state_dir.join("state.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(err) => {
                return Err(ApplyError::State(format!(
                    "cannot read {}: {err}",
                    path.display()
                )))
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(data)) => Ok(State { data }),
            _ => Err(ApplyError::State(format!(
                "{} is not a valid state file. Fix it, or delete it - but note \
                 that deleting it means steamtrain no longer recognises the \
                 launch options it set, so `steamtrain revert` will leave them \
                 in place.",
                path.display()
            ))),
        }
    }

    pub fn save(&self, state_dir: &Path) -> Result<(), ApplyError> {
        std::fs::create_dir_all(state_dir).map_err(|err| {
            ApplyError::Io(format!("cannot create {}: {err}", state_dir.display()))
        })?;
        let path = state_dir.join("state.json");
        let mut text = serde_json::to_string_pretty(&Value::Object(self.data.clone()))
            .map_err(|err| ApplyError::Io(format!("cannot serialize state: {err}")))?;
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|err| ApplyError::Io(format!("cannot write {}: {err}", path.display())))
    }

    pub fn get(&self, user: &str, appid: &str) -> Option<&str> {
        self.data
            .get(&format!("{user}/{appid}"))
            .and_then(Value::as_str)
    }

    pub fn record(&mut self, user: &str, appid: &str, value: &str) {
        let key = format!("{user}/{appid}");
        if value.is_empty() {
            // shift_remove, not remove: with preserve_order the latter swaps
            // the last entry into the hole and reorders the file.
            self.data.shift_remove(&key);
        } else {
            self.data.insert(key, Value::from(value));
        }
    }

    pub fn data(&self) -> &Map<String, Value> {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

fn load(path: &Path) -> Result<vdf::Block, ApplyError> {
    let bytes = std::fs::read(path)
        .map_err(|err| ApplyError::Io(format!("cannot read {}: {err}", path.display())))?;
    vdf::loads(&bytes).map_err(|err| ApplyError::Vdf(format!("{}: {err}", path.display())))
}

/// Walk to the apps block, creating any missing level with canonical casing.
/// Recursive rather than a loop so the mutable reborrow is unambiguous.
fn apps_node<'a>(node: &'a mut vdf::Block, path: &[&[u8]]) -> &'a mut vdf::Block {
    match path.split_first() {
        None => node,
        Some((head, rest)) => apps_node(node.child_ci(head), rest),
    }
}

/// Every appid's current LaunchOptions, from one parse of the file.
///
/// The Python re-parsed the whole file once per appid, which on a large
/// library meant hundreds of full parses of a multi-megabyte file per run.
fn read_launch_options(localconfig: &Path) -> Result<BTreeMap<String, String>, ApplyError> {
    let mut data = load(localconfig)?;
    let apps = apps_node(&mut data, &APPS_PATH);
    let mut out = BTreeMap::new();
    for (appid, value) in apps.iter() {
        let Some(block) = value.as_block() else {
            continue;
        };
        let options = block.get_str(b"LaunchOptions").unwrap_or(b"");
        out.insert(
            String::from_utf8_lossy(appid).into_owned(),
            String::from_utf8_lossy(options).into_owned(),
        );
    }
    Ok(out)
}

fn decide(current: &str, proposed: &str, last_written: Option<&str>) -> Action {
    if current == proposed {
        return Action::SkipUnchanged;
    }
    if current.is_empty() || last_written == Some(current) {
        return Action::Set;
    }
    Action::SkipUserSet
}

/// Plan per-user changes for every (appid -> proposed options).
///
/// Ordering is (user, then appid) and deterministic. The Python emitted them
/// in library-discovery order, which varied with how many libraries were
/// mounted; nothing consumes the order - the desktop interface sorts rows
/// itself - and sorted is reproducible.
pub fn plan_changes(
    root: &Path,
    options_by_appid: &BTreeMap<String, String>,
    state: &State,
    names: &BTreeMap<String, String>,
) -> Result<Vec<Change>, ApplyError> {
    let mut changes = Vec::new();
    for (user, localconfig) in steam::user_localconfigs(root) {
        let current_by_appid = read_launch_options(&localconfig)?;
        for (appid, proposed) in options_by_appid {
            let current = current_by_appid.get(appid).cloned().unwrap_or_default();
            let action = decide(&current, proposed, state.get(&user, appid));
            changes.push(Change {
                user: user.clone(),
                appid: appid.clone(),
                name: names.get(appid).cloned().unwrap_or_else(|| appid.clone()),
                current,
                proposed: proposed.clone(),
                action,
            });
        }
    }
    Ok(changes)
}

/// Plan restoring every still-ours managed option back to empty.
pub fn plan_revert(root: &Path, state: &State) -> Result<Vec<Change>, ApplyError> {
    let mut changes = Vec::new();
    for (user, localconfig) in steam::user_localconfigs(root) {
        let current_by_appid = read_launch_options(&localconfig)?;
        for (key, value) in state.data() {
            let Some((owner, appid)) = key.split_once('/') else {
                continue;
            };
            if owner != user {
                continue;
            }
            let written = value.as_str().unwrap_or("");
            let current = current_by_appid.get(appid).cloned().unwrap_or_default();
            let action = if current == written {
                Action::Set
            } else {
                Action::SkipUserSet
            };
            changes.push(Change {
                user: user.clone(),
                appid: appid.to_string(),
                // Revert plans against state, which can hold appids that are
                // no longer installed and so have no name to show.
                name: appid.to_string(),
                current,
                proposed: String::new(),
                action,
            });
        }
    }
    Ok(changes)
}

fn backup(localconfig: &Path, user: &str, state_dir: &Path) -> Result<(), ApplyError> {
    let backups = state_dir.join("backups");
    std::fs::create_dir_all(&backups)
        .map_err(|err| ApplyError::Io(format!("cannot create {}: {err}", backups.display())))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let target = backups.join(format!("localconfig-{user}-{stamp}.vdf"));
    std::fs::copy(localconfig, &target)
        .map_err(|err| ApplyError::Io(format!("cannot back up to {}: {err}", target.display())))?;
    prune_backups(&backups, user);
    Ok(())
}

/// Keep the newest BACKUPS_PER_USER for this account. Sorting by filename is
/// sorting by time: the stamp is a fixed-width nanosecond count.
fn prune_backups(backups: &Path, user: &str) {
    let prefix = format!("localconfig-{user}-");
    let Ok(entries) = std::fs::read_dir(backups) else {
        return;
    };
    let mut mine: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".vdf"))
        })
        .collect();
    mine.sort();
    let surplus = mine.len().saturating_sub(BACKUPS_PER_USER);
    for stale in mine.into_iter().take(surplus) {
        let _ = std::fs::remove_file(stale);
    }
}

fn write_atomic(localconfig: &Path, text: &[u8]) -> Result<(), ApplyError> {
    let mut name = localconfig
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".steamtrain-tmp");
    let tmp = localconfig.with_file_name(name);

    std::fs::write(&tmp, text)
        .map_err(|err| ApplyError::Io(format!("cannot write {}: {err}", tmp.display())))?;
    // Permissions are preserved. Timestamps deliberately are not: Python's
    // shutil.copystat carried the old mtime across, which made a file that had
    // just been rewritten look untouched to every backup tool on the machine.
    if let Ok(meta) = std::fs::metadata(localconfig) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, localconfig)
        .map_err(|err| ApplyError::Io(format!("cannot replace {}: {err}", localconfig.display())))
}

/// Execute planned 'set' changes. Returns the changes actually written.
pub fn apply_changes(
    root: &Path,
    changes: &[Change],
    state_dir: &Path,
    is_running: &dyn Fn(&Path) -> bool,
) -> Result<Vec<Change>, ApplyError> {
    let to_set: Vec<&Change> = changes
        .iter()
        .filter(|change| change.action == Action::Set)
        .collect();
    if to_set.is_empty() {
        return Ok(Vec::new());
    }
    if is_running(root) {
        return Err(ApplyError::SteamRunning(
            "Steam is running; localconfig.vdf would be overwritten on Steam \
             exit. Close Steam and re-run (the timer retries automatically)."
                .to_string(),
        ));
    }

    let mut state = State::load(state_dir)?;
    let localconfigs: BTreeMap<String, PathBuf> =
        steam::user_localconfigs(root).into_iter().collect();

    let mut users: Vec<&str> = to_set.iter().map(|change| change.user.as_str()).collect();
    users.sort_unstable();
    users.dedup();

    for user in users {
        let Some(localconfig) = localconfigs.get(user) else {
            continue;
        };
        backup(localconfig, user, state_dir)?;
        let mut data = load(localconfig)?;
        {
            let apps = apps_node(&mut data, &APPS_PATH);
            for change in to_set.iter().filter(|change| change.user == user) {
                apps.child_ci(change.appid.as_bytes()).insert(
                    b"LaunchOptions".to_vec(),
                    vdf::Value::Str(change.proposed.as_bytes().to_vec()),
                );
            }
        }
        for change in to_set.iter().filter(|change| change.user == user) {
            state.record(user, &change.appid, &change.proposed);
        }
        write_atomic(localconfig, &vdf::dumps(&data))?;
    }

    state.save(state_dir)?;
    Ok(to_set.into_iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_covers_the_three_outcomes() {
        assert_eq!(decide("x", "x", None), Action::SkipUnchanged);
        assert_eq!(decide("", "x", None), Action::Set);
        assert_eq!(decide("ours", "new", Some("ours")), Action::Set);
        assert_eq!(decide("theirs", "new", Some("ours")), Action::SkipUserSet);
        assert_eq!(decide("theirs", "new", None), Action::SkipUserSet);
    }

    #[test]
    fn recording_an_empty_value_forgets_the_key_without_reordering() {
        let mut state = State::default();
        state.record("111", "a", "1");
        state.record("111", "b", "2");
        state.record("111", "c", "3");
        state.record("111", "b", "");

        let keys: Vec<&str> = state.data().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["111/a", "111/c"]);
    }

    #[test]
    fn a_missing_state_file_is_an_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(State::load(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_state_file_is_an_error_not_an_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("state.json"), "{not json").unwrap();

        let err = State::load(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("state.json"), "got {err}");
        assert!(
            err.contains("revert"),
            "the consequence is spelled out: {err}"
        );
    }
}
