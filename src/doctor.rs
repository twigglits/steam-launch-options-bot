//! Diagnose the environment, and repair the one problem that has a repair.
//!
//! The problem this exists for is silent. A pre-package install created by
//! install.sh lives in ~/.local, and it wins over a packaged install three
//! independent ways, none of which produces a warning:
//!
//!   1. ~/.local/bin precedes /usr/bin in PATH on mainstream distributions, so
//!      `steamtrain` in a terminal resolves to the legacy copy.
//!   2. ~/.config/systemd/user/ takes precedence over /usr/lib/systemd/user/,
//!      so a legacy unit fully masks the packaged one of the same name.
//!   3. The legacy unit hardcodes ExecStart=%h/.local/bin/steamtrain, so even
//!      with precedence resolved the scheduled run executes the legacy code.
//!
//! Net effect: install the package, and none of it runs. Detection is
//! read-only and works with no systemd session. Repair removes executables and
//! unit files from a fixed allowlist and never touches configuration or state -
//! losing state.json would permanently strand every option the legacy install
//! wrote, because the tool would stop recognising those values as its own and
//! refuse to revert them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::codes::Guardrail;
use crate::proc::{self, CommandRunner};

pub const PACKAGED_BIN: &str = "/usr/bin/steamtrain";

const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: Guardrail,
    pub message: String,
    pub paths: Vec<String>,
    pub fixable: bool,
}

/// What `diagnose` is allowed to look at. Every field is injectable because
/// the answer depends entirely on the filesystem and PATH.
#[derive(Debug, Clone)]
pub struct Options {
    pub home: Option<PathBuf>,
    pub path_env: Option<String>,
    pub packaged_bin: PathBuf,
    /// Skip the "is there a package to shadow?" gate. The one caller that
    /// means it is `install.sh --migrate`, where the user has explicitly asked
    /// to clear the old install before a package exists.
    pub force: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            home: None,
            path_env: None,
            packaged_bin: PathBuf::from(PACKAGED_BIN),
            force: false,
        }
    }
}

impl Options {
    pub fn home(&self) -> PathBuf {
        self.home.clone().unwrap_or_else(crate::home)
    }
}

/// The complete set of paths migration is ever permitted to delete.
///
/// A fixed allowlist rather than a glob: `doctor --fix` must delete what it
/// came for and nothing it merely found nearby.
pub fn removable_paths(home: &Path) -> Vec<PathBuf> {
    let units = home.join(".config").join("systemd").join("user");
    vec![
        home.join(".local").join("lib").join("steamtrain"),
        home.join(".local").join("bin").join("steamtrain"),
        units.join("steamtrain.service"),
        units.join("steamtrain.timer"),
        units.join("timers.target.wants").join("steamtrain.timer"),
    ]
}

/// Never removable, at any point, by anything in this module.
pub fn protected_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config").join("steamtrain"),
        home.join(".local").join("state").join("steamtrain"),
    ]
}

/// `exists()` follows symlinks, so a dangling one would go unnoticed - and a
/// dangling ~/.local/bin/steamtrain still wins the PATH lookup.
fn present(path: &Path) -> bool {
    path.exists() || path.symlink_metadata().is_ok()
}

/// True when a PATH lookup for `steamtrain` lands on the legacy copy.
fn shadows_packaged_bin(legacy_bin: &Path, path_env: Option<&str>) -> bool {
    if !present(legacy_bin) {
        return false;
    }
    let Some(found) = proc::which("steamtrain", path_env) else {
        return false;
    };
    match (found.canonicalize(), legacy_bin.canonicalize()) {
        (Ok(resolved), Ok(legacy)) => resolved == legacy,
        _ => found == legacy_bin,
    }
}

/// Legacy paths present on this machine, or [] when there is no conflict.
///
/// Returns nothing unless a packaged install actually exists: with no package
/// there is nothing being shadowed, and an install.sh user who has not
/// switched yet must not be nagged about a problem they do not have. That gate
/// also stops `doctor --fix` deleting a working ~/.local install out from
/// under a developer running from a checkout.
pub fn find_legacy(options: &Options) -> Vec<PathBuf> {
    if !options.force && !options.packaged_bin.exists() {
        return Vec::new();
    }
    let home = options.home();
    let legacy_bin = home.join(".local").join("bin").join("steamtrain");
    let found: Vec<PathBuf> = removable_paths(&home)
        .into_iter()
        .filter(|path| present(path))
        .collect();
    if found.is_empty() {
        return Vec::new();
    }
    // A legacy binary that does not win the PATH lookup is inert; the unit
    // files and library directory are conflicts regardless.
    if !options.force
        && found.len() == 1
        && found[0] == legacy_bin
        && !shadows_packaged_bin(&legacy_bin, options.path_env.as_deref())
    {
        return Vec::new();
    }
    found
}

/// Every problem found. Read-only; safe with no systemd session.
pub fn diagnose(options: &Options) -> Vec<Finding> {
    let legacy = find_legacy(options);
    if legacy.is_empty() {
        return Vec::new();
    }
    vec![Finding {
        code: Guardrail::LegacyInstallShadowing,
        message: "an old user-level install is shadowing the packaged one, so \
                  the package you installed is not the code being run"
            .to_string(),
        paths: legacy
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        fixable: true,
    }]
}

/// Stop the legacy timer before its unit files vanish underneath systemd.
/// Best-effort: no systemd session is a normal state here, not a failure.
fn disable_legacy_timer(runner: &dyn CommandRunner) {
    let argv: Vec<String> = [
        "systemctl",
        "--user",
        "disable",
        "--now",
        "steamtrain.timer",
    ]
    .iter()
    .map(|part| part.to_string())
    .collect();
    let _ = runner.run(&argv, None, SYSTEMCTL_TIMEOUT);
}

/// What happened to one path: (path, detail).
pub type PathOutcome = (String, String);

/// (removed, failed).
pub type MigrationReport = (Vec<PathOutcome>, Vec<PathOutcome>);

/// Remove the legacy install.
///
/// Both halves name every path they cover, so a partial failure can say
/// exactly what was and was not done. Configuration and state are never
/// candidates - they are not in the allowlist, and the guard below rejects
/// them even if a future caller tries.
pub fn migrate(home: &Path, dry_run: bool, runner: &dyn CommandRunner) -> MigrationReport {
    let allowed = removable_paths(home);
    let protected = protected_paths(home);
    let mut removed = Vec::new();
    let mut failed = Vec::new();

    if !dry_run {
        disable_legacy_timer(runner);
    }

    for path in allowed {
        // Unreachable with the allowlist as it stands - a unit test asserts
        // the two sets are disjoint - and kept so that changing the allowlist
        // cannot quietly make it reachable.
        if protected
            .iter()
            .any(|guard| path == *guard || path.starts_with(guard))
        {
            debug_assert!(
                false,
                "{} is protected and must never be removed",
                path.display()
            );
            failed.push((path.display().to_string(), "protected; refused".to_string()));
            continue;
        }
        if !present(&path) {
            continue;
        }
        if dry_run {
            removed.push((path.display().to_string(), "would remove".to_string()));
            continue;
        }
        let is_real_dir = path
            .symlink_metadata()
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        let outcome = if is_real_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match outcome {
            Ok(()) => removed.push((path.display().to_string(), "removed".to_string())),
            Err(err) => failed.push((path.display().to_string(), err.to_string())),
        }
    }
    (removed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_paths_are_disjoint_from_removable_ones() {
        let home = Path::new("/home/example");
        for guard in protected_paths(home) {
            for candidate in removable_paths(home) {
                assert!(candidate != guard, "{candidate:?} is protected");
                assert!(
                    !candidate.starts_with(&guard),
                    "{candidate:?} is under {guard:?}"
                );
            }
        }
    }

    #[test]
    fn the_allowlist_is_exactly_five_paths() {
        // Growing it is a deliberate act, not an accident.
        assert_eq!(removable_paths(Path::new("/home/example")).len(), 5);
    }
}
