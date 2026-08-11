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
    from steamtrain_gui import client, models, system, window


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
        self.assertFalse(win.timer_checkbox.isEnabled())

    def test_progress_records_drive_the_progress_bar(self):
        win = self.make()
        win._on_record({"kind": "progress", "done": 12, "total": 24})
        self.assertEqual(24, win.progress.maximum())
        self.assertEqual(12, win.progress.value())

    def test_versions_of_both_halves_are_shown(self):
        win = self.make()
        self.assertIn("0.5.0", win.version_label.text())


@unittest.skipUnless(HAVE_QT, "PyQt6 not installed")
class SchedulingRowTest(unittest.TestCase):
    """The window must never claim a timer is running when it is not."""

    def make(self, state):
        win = window.MainWindow("0.5.0", "0.5.0")
        self.addCleanup(win.runner.wait)
        self.addCleanup(win.deleteLater)
        original = system.timer_state
        system.timer_state = lambda: state
        self.addCleanup(lambda: setattr(system, "timer_state", original))
        win._refresh_timer_row()
        return win

    def state(self, **kwargs):
        fields = dict(session=True, installed=True, enabled=False,
                      active=False, next_run=None, spent=False)
        fields.update(kwargs)
        return system.TimerState(**fields)

    def test_running_timer_is_ticked_and_names_the_next_run(self):
        win = self.make(self.state(enabled=True, active=True,
                                   next_run="Fri 2026-07-24 16:30:00 SAST"))
        self.assertTrue(win.timer_checkbox.isChecked())
        self.assertIn("16:30", win.timer_detail.text())

    def test_enabled_but_inactive_is_not_shown_as_running(self):
        """The state that would otherwise be a silent lie: ticked, never fires."""
        win = self.make(self.state(enabled=True, active=False))
        self.assertFalse(win.timer_checkbox.isChecked())
        self.assertIn("not counting down", win.timer_detail.text())

    def test_elapsed_timer_is_not_dressed_up_as_a_working_one(self):
        win = self.make(self.state(enabled=True, active=True, spent=True))
        self.assertIn("no further run is scheduled", win.timer_detail.text())

    def test_off_says_nothing_runs_on_its_own(self):
        win = self.make(self.state())
        self.assertFalse(win.timer_checkbox.isChecked())
        self.assertIn("nothing runs on its own", win.timer_detail.text())

    def test_missing_unit_disables_the_switch_rather_than_failing_on_click(self):
        win = self.make(self.state(installed=False))
        self.assertFalse(win.timer_checkbox.isEnabled())
        self.assertIn("not installed", win.timer_detail.text())

    def test_no_user_session_disables_the_switch(self):
        win = self.make(self.state(session=False, installed=False))
        self.assertFalse(win.timer_checkbox.isEnabled())


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
