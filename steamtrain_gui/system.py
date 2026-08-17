"""Cleanup of the systemd timer steamtrain used to install.

Scheduled runs now live inside the settings window and last exactly as long as
it is open, so a timer left enabled by an older install is the one way
steamtrain could still write to Steam while nothing of it is on screen.

Done once, not at every launch: `steamtrain.timer` is a name a user can also
write for themselves (the README shows how, for a headless machine), and a
window that silently switched their own unit off every time it opened would be
worse than the state it is cleaning up.

Kept free of Qt so it can be tested headlessly.
"""

import subprocess

LEGACY_TIMER = "steamtrain.timer"


def _run(argv, timeout=15):
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)


def disable_legacy_timer(runner=None):
    """Best effort, and deliberately silent.

    Every failure mode here is a machine on which the timer cannot be running
    either — no systemd user session, no such unit, systemctl absent — so there
    is nothing to report to the user.
    """
    runner = runner or _run
    try:
        runner(["systemctl", "--user", "disable", "--now", LEGACY_TIMER])
    except (OSError, subprocess.SubprocessError):
        pass
