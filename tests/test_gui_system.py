"""Per-user timer control. Qt-free, headless."""

import subprocess
import unittest

from steamtrain_gui import system


class Fake:
    def __init__(self, stdout="", returncode=0, stderr=""):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = stderr


SHOW_ENABLED = (
    "LoadState=loaded\n"
    "UnitFileState=enabled\n"
    "ActiveState=active\n"
    "NextElapseUSecRealtime=Fri 2026-07-24 16:30:00 SAST\n"
)
SHOW_DISABLED = (
    "LoadState=loaded\n"
    "UnitFileState=disabled\n"
    "ActiveState=inactive\n"
    "NextElapseUSecRealtime=n/a\n"
)
SHOW_ENABLED_NOT_STARTED = (
    "LoadState=loaded\n"
    "UnitFileState=enabled\n"
    "ActiveState=inactive\n"
    "NextElapseUSecRealtime=n/a\n"
)
SHOW_ABSENT = (
    "LoadState=not-found\n"
    "UnitFileState=\n"
    "ActiveState=inactive\n"
    "NextElapseUSecRealtime=\n"
)


class TimerStateTest(unittest.TestCase):
    def test_enabled_timer_reports_its_next_run(self):
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(SHOW_ENABLED))
        self.assertTrue(state.session)
        self.assertTrue(state.enabled)
        self.assertTrue(state.active)
        self.assertTrue(state.running)
        self.assertEqual("Fri 2026-07-24 16:30:00 SAST", state.next_run)
        self.assertIn("Fri 2026-07-24 16:30:00 SAST", state.describe())

    def test_disabled_timer_has_no_next_run(self):
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(SHOW_DISABLED))
        self.assertTrue(state.session)
        self.assertFalse(state.enabled)
        self.assertFalse(state.running)
        self.assertIsNone(state.next_run)
        self.assertIn("nothing runs on its own", state.describe())

    def test_enabled_but_never_started_is_not_running(self):
        """Enabled is what happens next boot; it is not "a run is scheduled"."""
        state = system.timer_state(
            runner=lambda argv, timeout=15: Fake(SHOW_ENABLED_NOT_STARTED))
        self.assertTrue(state.enabled)
        self.assertFalse(state.active)
        self.assertFalse(state.running)

    def test_absent_unit_is_reported_apart_from_being_switched_off(self):
        """A CLI-only install has no unit; the switch cannot turn it on."""
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(SHOW_ABSENT))
        self.assertTrue(state.session)
        self.assertFalse(state.installed)
        self.assertFalse(state.controllable)
        self.assertIn("not installed", state.describe())

    def test_no_user_bus_is_a_normal_state_not_an_error(self):
        """A container or a plain ssh session has no user bus; the CLI still works."""
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(
            "", 1, "Failed to connect to bus: No such file or directory"))
        self.assertFalse(state.session)
        self.assertFalse(state.controllable)

    def test_missing_systemctl_binary_degrades_rather_than_raising(self):
        def missing(argv, timeout=15):
            raise FileNotFoundError("systemctl")
        state = system.timer_state(runner=missing)
        self.assertFalse(state.session)

    def test_systemctl_timeout_degrades_rather_than_raising(self):
        def slow(argv, timeout=15):
            raise subprocess.TimeoutExpired("systemctl", timeout)
        state = system.timer_state(runner=slow)
        self.assertFalse(state.session)


class SetTimerTest(unittest.TestCase):
    def test_enable_uses_user_scope_and_starts_immediately(self):
        seen = {}

        def runner(argv, timeout=15):
            seen["argv"] = argv
            return Fake()

        ok, _ = system.set_timer(True, runner=runner)
        self.assertTrue(ok)
        self.assertEqual(
            ["systemctl", "--user", "enable", "--now", "steamtrain.timer"],
            seen["argv"])

    def test_disable_stops_it_too(self):
        seen = {}

        def runner(argv, timeout=15):
            seen["argv"] = argv
            return Fake()

        system.set_timer(False, runner=runner)
        self.assertIn("disable", seen["argv"])
        self.assertIn("--now", seen["argv"])

    def test_never_uses_global_scope(self):
        """AD-9: enabling for every user on the machine is not ours to do."""
        seen = {}
        system.set_timer(True, runner=lambda argv, timeout=15: (
            seen.update(argv=argv) or Fake()))
        self.assertNotIn("--global", seen["argv"])

    def test_failure_returns_the_reason_for_display(self):
        ok, message = system.set_timer(True, runner=lambda argv, timeout=15: Fake(
            "", 1, "Unit steamtrain.timer is masked."))
        self.assertFalse(ok)
        self.assertIn("masked", message)


if __name__ == "__main__":
    unittest.main()
