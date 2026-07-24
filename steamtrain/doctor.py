"""Diagnose the environment, and repair the one problem that has a repair.

The problem this exists for is silent. A pre-package install created by
install.sh lives in ~/.local, and it wins over a packaged install three
independent ways, none of which produces a warning:

  1. ~/.local/bin precedes /usr/bin in PATH on mainstream distributions, so
     `steamtrain` in a terminal resolves to the legacy copy.
  2. ~/.config/systemd/user/ takes precedence over /usr/lib/systemd/user/, so
     a legacy unit fully masks the packaged one of the same name.
  3. The legacy unit hardcodes ExecStart=%h/.local/bin/steamtrain, so even
     with precedence resolved the scheduled run executes the legacy code.

Net effect: install the package, and none of it runs. Detection is read-only
and works with no systemd session. Repair removes executables and unit files
from a fixed allowlist and never touches configuration or state - losing
state.json would permanently strand every option the legacy install wrote,
because the tool would stop recognising those values as its own and refuse to
revert them.
"""

import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from . import codes

PACKAGED_BIN = Path("/usr/bin/steamtrain")


@dataclass
class Finding:
    code: str
    message: str
    paths: list = field(default_factory=list)
    fixable: bool = False


def _home(home):
    return Path(home) if home is not None else Path.home()


def removable_paths(home=None):
    """The complete set of paths migration is ever permitted to delete.

    A fixed allowlist rather than a glob: `doctor --fix` must delete what it
    came for and nothing it merely found nearby.
    """
    home = _home(home)
    units = home / ".config" / "systemd" / "user"
    return [
        home / ".local" / "lib" / "steamtrain",
        home / ".local" / "bin" / "steamtrain",
        units / "steamtrain.service",
        units / "steamtrain.timer",
        units / "timers.target.wants" / "steamtrain.timer",
    ]


def protected_paths(home=None):
    """Never removable, at any point, by anything in this module."""
    home = _home(home)
    return [
        home / ".config" / "steamtrain",
        home / ".local" / "state" / "steamtrain",
    ]


def _shadows_packaged_bin(legacy_bin, path_env, packaged_bin):
    """True when a PATH lookup for `steamtrain` lands on the legacy copy."""
    if not legacy_bin.exists():
        return False
    found = shutil.which("steamtrain", path=path_env)
    return found is not None and Path(found).resolve() == legacy_bin.resolve()


def find_legacy(home=None, path_env=None, packaged_bin=PACKAGED_BIN, force=False):
    """Legacy paths present on this machine, or [] when there is no conflict.

    Returns nothing unless a packaged install actually exists: with no package
    there is nothing being shadowed, and an install.sh user who has not
    switched yet must not be nagged about a problem they do not have. That
    gate also stops `doctor --fix` deleting a working ~/.local install out
    from under a developer running from a checkout.

    `force` skips the gate for the one caller that means it: `install.sh
    --migrate`, where the user has explicitly asked to clear the old install
    before a package exists to shadow it.
    """
    packaged_bin = Path(packaged_bin)
    if not force and not packaged_bin.exists():
        return []
    home = _home(home)
    legacy_bin = home / ".local" / "bin" / "steamtrain"
    found = [p for p in removable_paths(home) if p.exists() or p.is_symlink()]
    if not found:
        return []
    # A legacy binary that does not win the PATH lookup is inert; the unit
    # files and library directory are conflicts regardless.
    if (not force and found == [legacy_bin]
            and not _shadows_packaged_bin(legacy_bin, path_env, packaged_bin)):
        return []
    return found


def diagnose(home=None, path_env=None, packaged_bin=PACKAGED_BIN, force=False):
    """Every problem found. Read-only; safe with no systemd session."""
    findings = []
    legacy = find_legacy(home, path_env, packaged_bin, force)
    if legacy:
        findings.append(Finding(
            code=codes.LEGACY_INSTALL_SHADOWING,
            message=(
                "an old user-level install is shadowing the packaged one, so "
                "the package you installed is not the code being run"),
            paths=[str(p) for p in legacy],
            fixable=True,
        ))
    return findings


class RemovalRefused(Exception):
    """A path outside the allowlist was passed to the remover."""


def _disable_legacy_timer(runner):
    """Stop the legacy timer before its unit files vanish underneath systemd.

    Best-effort: no systemd session is a normal state here, not a failure.
    """
    try:
        runner(["systemctl", "--user", "disable", "--now", "steamtrain.timer"])
    except (OSError, subprocess.SubprocessError):
        pass


def _default_runner(argv):
    subprocess.run(argv, check=False, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)


def migrate(home=None, dry_run=False, runner=_default_runner):
    """Remove the legacy install. Returns (removed, failed).

    `removed` and `failed` are (path, detail) pairs so a partial failure can
    name exactly what was and was not done. Configuration and state are never
    candidates - they are not in the allowlist, and a guard rejects them even
    if a future caller tries.
    """
    home = _home(home)
    allowed = removable_paths(home)
    protected = protected_paths(home)
    removed, failed = [], []

    if not dry_run:
        _disable_legacy_timer(runner)

    for path in allowed:
        for guard in protected:
            if path == guard or guard in path.parents:
                raise RemovalRefused(f"{path} is protected and must never be removed")
        if not (path.exists() or path.is_symlink()):
            continue
        if dry_run:
            removed.append((str(path), "would remove"))
            continue
        try:
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
            removed.append((str(path), "removed"))
        except OSError as exc:
            failed.append((str(path), str(exc)))
    return removed, failed
