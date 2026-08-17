"""Settings window and table model. Headless via QT_QPA_PLATFORM=offscreen.

These run without a display, and deliberately without an event loop, so the
window's deferred initial refresh never fires and no real steamtrain process
is spawned.
"""

import os
import unittest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from tests.qtapp import HAVE_QT, ensure_app

if HAVE_QT:
    from steamtrain_gui import client, models, window


def setUpModule():
    ensure_app()


def run_with(*records):
    return client.Run(list(records), 0)


def change(user="111", appid="100", action="set", current="", proposed="x"):
    return {"kind": "change", "user": user, "appid": appid, "action": action,
            "current": current, "proposed": proposed}


def game(appid="100", name="Fixture Game", runtime="proton"):
    return {"kind": "game", "appid": appid, "name": name, "runtime": runtime}


def result(**fields):
    base = {"kind": "result", "ok": True, "outcome": "ok"}
    base.update(fields)
    return base


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class RowsTest(unittest.TestCase):
    def test_change_joins_to_its_game_record_for_the_name(self):
        rows = models.rows_from_run(run_with(game(), change(), result()))
        self.assertEqual("Fixture Game", rows[0].name)
        self.assertEqual("proton", rows[0].runtime)

    def test_change_without_a_game_record_falls_back_to_the_appid(self):
        """Expected after a revert: state can hold uninstalled appids."""
        rows = models.rows_from_run(run_with(change(appid="999"), result()))
        self.assertEqual("999", rows[0].name)

    def test_one_row_per_user_appid_pair(self):
        rows = models.rows_from_run(run_with(
            game(), change(user="111"), change(user="222"), result()))
        self.assertEqual(2, len(rows))
        self.assertEqual({"111", "222"}, {r.user for r in rows})

    def test_unknown_action_is_shown_verbatim_not_blanked(self):
        rows = models.rows_from_run(run_with(change(action="teleported"), result()))
        self.assertEqual("teleported", rows[0].status_text)
        self.assertIn("does not recognise", rows[0].status_tooltip)

    def test_every_known_action_has_words_not_just_colour(self):
        for action in ("set", "skip-unchanged", "skip-user-set", "excluded"):
            with self.subTest(action=action):
                rows = models.rows_from_run(run_with(change(action=action), result()))
                self.assertTrue(rows[0].status_text.strip())
                self.assertTrue(rows[0].status_tooltip.strip())


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class ModelTest(unittest.TestCase):
    def setUp(self):
        self.model = models.GameTableModel()

    def test_model_is_read_only(self):
        self.model.set_rows(models.rows_from_run(run_with(game(), change(), result())))
        from PyQt6.QtCore import Qt
        flags = self.model.flags(self.model.index(0, models.COL_PROPOSED))
        self.assertFalse(flags & Qt.ItemFlag.ItemIsEditable)

    def test_single_account_is_not_multi_account(self):
        self.model.set_rows(models.rows_from_run(run_with(game(), change(), result())))
        self.assertFalse(self.model.multi_account)

    def test_two_accounts_are_detected(self):
        self.model.set_rows(models.rows_from_run(run_with(
            game(), change(user="111"), change(user="222"), result())))
        self.assertTrue(self.model.multi_account)

    def test_empty_current_reads_as_empty_not_blank(self):
        self.model.set_rows(models.rows_from_run(run_with(game(), change(), result())))
        from PyQt6.QtCore import Qt
        value = self.model.data(self.model.index(0, models.COL_CURRENT),
                                Qt.ItemDataRole.DisplayRole)
        self.assertEqual("(empty)", value)

    def test_status_cell_has_accessible_text_naming_the_game(self):
        self.model.set_rows(models.rows_from_run(run_with(game(), change(), result())))
        from PyQt6.QtCore import Qt
        text = self.model.data(self.model.index(0, models.COL_STATUS),
                               Qt.ItemDataRole.AccessibleTextRole)
        self.assertIn("Fixture Game", text)


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class WindowTest(unittest.TestCase):
    def make(self):
        win = window.MainWindow("0.5.0", "0.5.0")
        self.addCleanup(win.runner.wait)
        self.addCleanup(win.deleteLater)
        return win

    def test_account_column_hidden_with_one_account(self):
        win = self.make()
        win._on_run_finished(run_with(game(), change(), result(counts={})))
        self.assertTrue(win.table.isColumnHidden(models.COL_ACCOUNT))

    def test_account_column_shown_with_two_accounts(self):
        win = self.make()
        win._on_run_finished(run_with(
            game(), change(user="111"), change(user="222"), result(counts={})))
        self.assertFalse(win.table.isColumnHidden(models.COL_ACCOUNT))

    def test_steam_running_blocks_apply_and_explains_why(self):
        win = self.make()
        win._on_run_finished(run_with(game(), change(),
                                      result(counts={}, steam_running=True)))
        self.assertFalse(win.apply_button.isEnabled())
        self.assertIn("Steam", win.apply_button.toolTip())
        self.assertFalse(win.banner.isHidden())

    def test_steam_closed_leaves_apply_available(self):
        win = self.make()
        win._on_run_finished(run_with(game(), change(),
                                      result(counts={}, steam_running=False)))
        self.assertTrue(win.apply_button.isEnabled())

    def test_dry_run_stays_available_while_steam_runs(self):
        """apply --dry-run writes nothing, so Steam being open does not matter."""
        win = self.make()
        win._on_run_finished(run_with(game(), change(),
                                      result(counts={}, steam_running=True)))
        self.assertTrue(win.dry_run_button.isEnabled())

    def test_blocked_result_shows_the_cores_own_message(self):
        win = self.make()
        win._on_run_finished(run_with(result(
            ok=False, outcome="blocked", guardrail="steam-running",
            message="Steam is running; close it and re-run.", counts={})))
        self.assertIn("close it and re-run", win.banner.text())

    def test_unknown_guardrail_still_reaches_the_user(self):
        win = self.make()
        win._on_run_finished(run_with(result(
            ok=False, outcome="blocked", guardrail="brand-new-reason",
            message="a reason from a newer core", counts={})))
        self.assertIn("newer core", win.banner.text())

    def test_failure_is_surfaced_not_swallowed(self):
        win = self.make()
        win._on_run_failed("steamtrain is not on PATH")
        self.assertFalse(win.banner.isHidden())
        self.assertIn("not on PATH", win.banner.text())

    def test_degraded_mode_disables_every_write_action(self):
        win = self.make()
        win.set_degraded("An old install is still in the way.")
        for button in (win.apply_button, win.revert_button,
                       win.dry_run_button, win.refresh_button):
            self.assertFalse(button.isEnabled())
        self.assertFalse(win.schedule.isActive())

    def test_progress_records_drive_the_progress_bar(self):
        win = self.make()
        win._on_record({"kind": "progress", "done": 12, "total": 24})
        self.assertEqual(24, win.progress.maximum())
        self.assertEqual(12, win.progress.value())

    def test_versions_of_both_halves_are_shown(self):
        win = self.make()
        self.assertIn("0.5.0", win.version_label.text())


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class SchedulingTest(unittest.TestCase):
    """Scheduled runs exist while this window does, and there is no switch.

    Opening the window is the switch, so the tests assert the timer itself:
    there is no stored preference that could disagree with it, and the row
    reads its words back from the timer rather than from an intention.
    """

    def make(self):
        win = window.MainWindow("0.5.0", "0.5.0")
        self.addCleanup(win.runner.wait)
        self.addCleanup(win.deleteLater)
        return win

    def test_an_open_window_is_already_scheduling(self):
        win = self.make()
        self.assertTrue(win.schedule.isActive())
        self.assertEqual(30 * 60 * 1000, win.schedule.interval())

    def test_the_row_says_it_needs_the_window_open(self):
        win = self.make()
        self.assertIn("this window", win.schedule_label.text())

    def test_nothing_offers_to_switch_scheduling_off(self):
        """A checkbox here would be a second answer to "is it running?"."""
        from PyQt6.QtWidgets import QCheckBox
        win = self.make()
        self.assertEqual([], win.findChildren(QCheckBox))

    def test_closing_the_window_ends_scheduled_runs(self):
        win = self.make()
        win.close()
        self.assertFalse(win.schedule.isActive())

    def test_a_tick_while_busy_is_skipped_rather_than_queued(self):
        win = self.make()
        started = []
        win._start = lambda *args: started.append(args)
        win.runner._busy = True   # as if a hand-driven run were in flight
        win._scheduled_run()
        self.assertEqual([], started)

    def test_a_tick_applies(self):
        win = self.make()
        started = []
        win._start = lambda *args: started.append(args)
        win._scheduled_run()
        self.assertEqual([(["apply"], "Writing launch options…")], started)

    def test_degraded_mode_stops_scheduling_and_says_so(self):
        win = self.make()
        win.set_degraded("An old install is still in the way.")
        self.assertFalse(win.schedule.isActive())
        self.assertIn("stopped", win.schedule_label.text())


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class FirstRunTest(unittest.TestCase):
    def test_defaults_to_autodetect(self):
        dialog = window.FirstRunDialog({"gpu_vendor": "nvidia"})
        self.addCleanup(dialog.deleteLater)
        self.assertEqual("auto", dialog.chosen_vendor())

    def test_offers_every_vendor_the_core_accepts(self):
        dialog = window.FirstRunDialog({"gpu_vendor": "unknown"})
        self.addCleanup(dialog.deleteLater)
        offered = {b.property("vendor") for b in dialog._buttons}
        self.assertEqual({"auto", "nvidia", "amd", "intel"}, offered)

    def test_selecting_a_vendor_is_reported(self):
        dialog = window.FirstRunDialog({"gpu_vendor": "unknown"})
        self.addCleanup(dialog.deleteLater)
        for button in dialog._buttons:
            if button.property("vendor") == "amd":
                button.setChecked(True)
        self.assertEqual("amd", dialog.chosen_vendor())


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class MigrationDialogTest(unittest.TestCase):
    def test_every_path_is_named_before_anything_is_removed(self):
        paths = ["/home/x/.local/bin/steamtrain", "/home/x/.local/lib/steamtrain"]
        dialog = window.MigrationDialog({"paths": paths})
        self.addCleanup(dialog.deleteLater)
        texts = [w.text() for w in dialog.findChildren(type(dialog.findChild(
            __import__("PyQt6.QtWidgets", fromlist=["QLabel"]).QLabel)))]
        joined = " ".join(texts)
        for path in paths:
            self.assertIn(path, joined)

    def test_states_that_config_and_state_are_preserved(self):
        dialog = window.MigrationDialog({"paths": []})
        self.addCleanup(dialog.deleteLater)
        from PyQt6.QtWidgets import QLabel
        joined = " ".join(label.text() for label in dialog.findChildren(QLabel))
        self.assertIn(".config/steamtrain", joined)
        self.assertIn(".local/state/steamtrain", joined)


if __name__ == "__main__":
    unittest.main()
