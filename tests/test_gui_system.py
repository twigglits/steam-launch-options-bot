"""Cleanup of the timer older releases installed. Qt-free, headless."""

import subprocess
import unittest

from steamtrain_gui import system

try:
    from steamtrain_gui import app
    HAVE_QT = True
except ImportError:  # the GUI package is optional
    HAVE_QT = False


class FakeSettings:
    """Stands in for QSettings: the same two calls, none of the Qt."""

    def __init__(self, **values):
        self.values = values

    def value(self, key, default=None, type=None):
        return self.values.get(key, default)

    def setValue(self, key, value):
        self.values[key] = value


class Fake:
    def __init__(self, stdout="", returncode=0, stderr=""):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = stderr


class LegacyTimerTest(unittest.TestCase):
    def test_switches_the_old_timer_off_for_this_user_only(self):
        calls = []

        def runner(argv, timeout=15):
            calls.append(argv)
            return Fake()

        system.disable_legacy_timer(runner=runner)
        self.assertEqual(
            [["systemctl", "--user", "disable", "--now", "steamtrain.timer"]],
            calls)

    def test_absent_systemctl_is_not_an_error(self):
        """No systemd here means no timer here: nothing to report."""
        def missing(argv, timeout=15):
            raise FileNotFoundError("systemctl")

        system.disable_legacy_timer(runner=missing)

    def test_a_hung_systemctl_does_not_hang_the_launch(self):
        def slow(argv, timeout=15):
            raise subprocess.TimeoutExpired(argv, timeout)

        system.disable_legacy_timer(runner=slow)

    def test_no_such_unit_is_not_an_error(self):
        """The common case: a user who never had the timer in the first place."""
        system.disable_legacy_timer(
            runner=lambda argv, timeout=15: Fake(
                "", 1, "Failed to disable: Unit file steamtrain.timer does not exist."))


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class LegacyTimerMigrationTest(unittest.TestCase):
    """Once, ever — not at every launch."""

    def setUp(self):
        self.calls = []
        original = system.disable_legacy_timer
        system.disable_legacy_timer = lambda: self.calls.append("disable")
        self.addCleanup(
            lambda: setattr(system, "disable_legacy_timer", original))

    def test_first_launch_switches_the_old_timer_off(self):
        settings = FakeSettings()
        self.assertTrue(app.migrate_legacy_timer(settings))
        self.assertEqual(["disable"], self.calls)

    def test_a_later_launch_leaves_a_timer_of_that_name_alone(self):
        """It could be the user's own unit by then, and theirs is theirs."""
        settings = FakeSettings()
        app.migrate_legacy_timer(settings)
        self.assertFalse(app.migrate_legacy_timer(settings))
        self.assertEqual(["disable"], self.calls)


if __name__ == "__main__":
    unittest.main()
