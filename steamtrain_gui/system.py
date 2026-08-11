"""Per-user systemd timer control.

Kept free of Qt so it can be tested headlessly. Everything here is scoped to
the invoking user: the packages install the unit files system-wide but enable
nothing, so switching the timer on is always this code acting on behalf of the
person who clicked it, never a maintainer script acting on everyone.
"""

import subprocess

TIMER_UNIT = "steamtrain.timer"


def _run(argv, timeout=15):
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)


class TimerState:
    """What the timer is actually doing, not what we asked it to do.

    `enabled` is what systemd will do at the next boot; `active` is whether the
    timer is loaded and counting right now. They can disagree - an enabled unit
    that was never started counts down to nothing - and the window says so
    rather than showing a ticked box for a timer that will not fire.
    """

    def __init__(self, session, installed, enabled, active, next_run):
        self.session = session      # is there a systemd user session at all
        self.installed = installed  # are the unit files present
        self.enabled = enabled
        self.active = active
        self.next_run = next_run    # human-readable, or None

    @property
    def controllable(self):
        return self.session and self.installed

    @property
    def running(self):
        """The one question the panel exists to answer."""
        return self.enabled and self.active

    def describe(self):
        """One line, plain language, for the window."""
        if not self.session:
            return ("no systemd user session here — nothing can run on a "
                    "schedule; press Apply when you want options written")
        if not self.installed:
            return (f"{TIMER_UNIT} is not installed — nothing runs on a "
                    f"schedule; press Apply when you want options written")
        if self.running:
            return f"running — next run {self.next_run}" if self.next_run else "running"
        if self.enabled:
            return "enabled for next login, but not counting down right now"
        return "off — nothing runs on its own; press Apply when you want a run"


def _offline(session=False, installed=False):
    return TimerState(session=session, installed=installed, enabled=False,
                      active=False, next_run=None)


def timer_state(runner=None):
    runner = runner or _run
    try:
        probe = runner(["systemctl", "--user", "show", TIMER_UNIT,
                        "--property=LoadState",
                        "--property=UnitFileState",
                        "--property=ActiveState",
                        "--property=NextElapseUSecRealtime"])
    except (OSError, subprocess.SubprocessError):
        return _offline()

    # No user bus is a normal state (a container, an ssh session, a distro
    # without lingering) rather than an error, and the CLI still works there.
    if probe.returncode != 0:
        return _offline()

    values = {}
    for line in probe.stdout.splitlines():
        key, _, value = line.partition("=")
        values[key] = value

    # A CLI-only install, or a checkout, has no unit files. That is not "the
    # timer is off" - the switch cannot turn it on - so it is reported apart.
    if values.get("LoadState", "") == "not-found":
        return _offline(session=True)

    next_run = values.get("NextElapseUSecRealtime", "").strip()
    if next_run in ("", "n/a", "0"):
        next_run = None

    return TimerState(
        session=True,
        installed=True,
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
