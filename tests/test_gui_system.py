"""Per-user timer control and the autostart entry. Qt-free, headless."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from steamtrain_gui import system


class Fake:
    def __init__(self, stdout="", returncode=0, stderr=""):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = stderr


SHOW_ENABLED = (
    "UnitFileState=enabled\n"
    "ActiveState=active\n"
    "NextElapseUSecRealtime=Fri 2026-07-24 16:30:00 SAST\n"
)
SHOW_DISABLED = (
    "UnitFileState=disabled\n"
    "ActiveState=inactive\n"
    "NextElapseUSecRealtime=n/a\n"
)


class TimerStateTest(unittest.TestCase):
    def test_enabled_timer_reports_its_next_run(self):
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(SHOW_ENABLED))
        self.assertTrue(state.session)
        self.assertTrue(state.enabled)
        self.assertTrue(state.active)
        self.assertEqual("Fri 2026-07-24 16:30:00 SAST", state.next_run)

    def test_disabled_timer_has_no_next_run(self):
        state = system.timer_state(runner=lambda argv, timeout=15: Fake(SHOW_DISABLED))
        self.assertTrue(state.session)
        self.assertFalse(state.enabled)
        self.assertIsNone(state.next_run)

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


class AutostartTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        patcher = mock.patch.dict("os.environ", {}, clear=False)
        patcher.start()
        self.addCleanup(patcher.stop)
        import os
        os.environ.pop("XDG_CONFIG_HOME", None)

    def test_absent_by_default(self):
        self.assertFalse(system.autostart_enabled(home=self.home))

    def test_enabling_writes_a_valid_entry(self):
        ok, _ = system.set_autostart(True, home=self.home)
        self.assertTrue(ok)
        path = system.autostart_path(home=self.home)
        self.assertTrue(path.is_file())
        body = path.read_text()
        self.assertIn("[Desktop Entry]", body)
        self.assertIn("Exec=steamtrain-gui --tray", body)

    def test_disabling_removes_it(self):
        system.set_autostart(True, home=self.home)
        system.set_autostart(False, home=self.home)
        self.assertFalse(system.autostart_enabled(home=self.home))

    def test_disabling_when_absent_is_not_an_error(self):
        ok, message = system.set_autostart(False, home=self.home)
        self.assertTrue(ok, message)

    def test_respects_xdg_config_home(self):
        import os
        other = Path(self.tmp.name) / "xdg"
        os.environ["XDG_CONFIG_HOME"] = str(other)
        try:
            system.set_autostart(True)
            self.assertTrue((other / "autostart" / system.AUTOSTART_NAME).is_file())
        finally:
            os.environ.pop("XDG_CONFIG_HOME")

    def test_nothing_is_ever_written_to_etc_xdg(self):
        """The package must ship no system-wide autostart; only the user opts in."""
        path = system.autostart_path(home=self.home)
        self.assertNotIn("/etc/xdg", str(path))
        self.assertIn(str(self.home), str(path))


if __name__ == "__main__":
    unittest.main()
