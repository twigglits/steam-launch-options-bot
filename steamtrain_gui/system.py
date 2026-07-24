"""Per-user systemd timer control and the autostart entry.

Kept free of Qt so it can be tested headlessly. Everything here is scoped to
the invoking user: the packages install the unit files system-wide but enable
nothing, so switching the timer on is always this code acting on behalf of the
person who clicked it, never a maintainer script acting on everyone.
"""

import os
import subprocess
from pathlib import Path

TIMER_UNIT = "steamtrain.timer"
AUTOSTART_NAME = "steamtrain-tray.desktop"

AUTOSTART_BODY = """\
[Desktop Entry]
Type=Application
Name=steamtrain tray
Comment=Show steamtrain in the system tray
Exec=steamtrain-gui --tray
Icon=steamtrain
Terminal=false
X-GNOME-Autostart-enabled=true
"""


def _run(argv, timeout=15):
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)


class TimerState:
    """What the timer is actually doing, not what we asked it to do."""

    def __init__(self, session, enabled, active, next_run):
        self.session = session      # is there a systemd user session at all
        self.enabled = enabled
        self.active = active
        self.next_run = next_run    # human-readable, or None

    @property
    def controllable(self):
        return self.session


def timer_state(runner=None):
    runner = runner or _run
    try:
        probe = runner(["systemctl", "--user", "show", TIMER_UNIT,
                        "--property=UnitFileState",
                        "--property=ActiveState",
                        "--property=NextElapseUSecRealtime"])
    except (OSError, subprocess.SubprocessError):
        return TimerState(session=False, enabled=False, active=False, next_run=None)

    # No user bus is a normal state (a container, an ssh session, a distro
    # without lingering) rather than an error, and the CLI still works there.
    if probe.returncode != 0:
        return TimerState(session=False, enabled=False, active=False, next_run=None)

    values = {}
    for line in probe.stdout.splitlines():
        key, _, value = line.partition("=")
        values[key] = value

    next_run = values.get("NextElapseUSecRealtime", "").strip()
    if next_run in ("", "n/a", "0"):
        next_run = None

    return TimerState(
        session=True,
        enabled=values.get("UnitFileState", "") == "enabled",
        active=values.get("ActiveState", "") == "active",
        next_run=next_run,
    )


def set_timer(enabled, runner=None):
    """Turn the timer on or off. Returns (ok, message).

    The caller must re-read timer_state() afterwards and display that, rather
    than assuming this worked: the switch has to reflect reality, not intent.
    """
    runner = runner or _run
    action = ["enable", "--now"] if enabled else ["disable", "--now"]
    try:
        done = runner(["systemctl", "--user", *action, TIMER_UNIT])
    except (OSError, subprocess.SubprocessError) as exc:
        return False, str(exc)
    if done.returncode != 0:
        return False, (done.stderr or done.stdout).strip() or "systemctl failed"
    return True, ""


def autostart_path(home=None):
    base = os.environ.get("XDG_CONFIG_HOME")
    if base:
        root = Path(base)
    else:
        root = Path(home) / ".config" if home is not None else Path.home() / ".config"
    return root / "autostart" / AUTOSTART_NAME


def autostart_enabled(home=None):
    return autostart_path(home).is_file()


def set_autostart(enabled, home=None):
    """Create or remove the autostart entry. Returns (ok, message).

    This file is the one thing the desktop interface writes directly. It is
    desktop-integration state that the Core has no business knowing about;
    everything else goes through the CLI.
    """
    path = autostart_path(home)
    try:
        if enabled:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(AUTOSTART_BODY, encoding="utf-8")
        elif path.exists():
            path.unlink()
    except OSError as exc:
        return False, str(exc)
    return True, ""
