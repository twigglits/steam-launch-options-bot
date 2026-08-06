"""The window and the CLI must plan identically.

This is the executable form of the rule that the desktop interface is a client
of the Core and never a second implementation of it. If the two ever disagree
about what would be written, the safety story is gone: the window would be
showing a plan the timer will not carry out.

Compares the two rendering paths of the same planner, not the two code paths,
because there is only one planner and that is the point.

The Core is a separate program now, so it is invoked as one. That is also the
honest way to test this claim: the interface reaches the Core by executing it,
and so does this test.
"""

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from steamtrain_gui import client

from tests.coreproc import CORE, SKIP_REASON, make_manifest, make_steam_root, make_user


@unittest.skipIf(CORE is None, SKIP_REASON)
class PlanEquivalenceTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.root = make_steam_root(base)
        for appid, name in (("100", "Fixture Game"), ("200", "Second Game"),
                            ("300", "Excluded Game")):
            make_manifest(self.root, appid, name, name.replace(" ", ""))
        for account in ("111", "222"):
            make_user(self.root, account)
        self.state_dir = base / "state"
        self.config = base / "config.json"
        self.config.write_text(json.dumps({"exclude": ["300"]}))

    def _run_cli(self, *args):
        done = subprocess.run(
            [CORE, *args,
             "--steam-root", str(self.root),
             "--state-dir", str(self.state_dir),
             "--config", str(self.config)],
            capture_output=True, text=True, timeout=120,
            # A hermetic HOME: autodetection and the legacy-install check must
            # not see whatever this machine happens to have.
            env={**os.environ, "HOME": self.tmp.name},
        )
        return done.returncode, done.stdout

    def _text_plan(self):
        """Parse the human output back into (user, appid, action) triples."""
        _, text = self._run_cli("apply", "--dry-run")
        markers = {"SET": "set", "ok": "skip-unchanged", "KEEP": "skip-user-set"}
        plan = set()
        for line in text.splitlines():
            found = re.match(r"\s*\[(\S+)\s*\]\s+user\s+(\S+)\s+(\S+)\s+", line)
            if found:
                marker, user, appid = found.groups()
                plan.add((user, appid, markers[marker]))
        return plan

    def _json_plan(self):
        _, text = self._run_cli("apply", "--dry-run", "--json")
        records = client.parse_stream(text)
        return {(r["user"], r["appid"], r["action"])
                for r in records if r.get("kind") == client.KIND_CHANGE
                and r["action"] != "excluded"}

    def test_both_renderings_describe_the_same_plan(self):
        self.assertEqual(self._text_plan(), self._json_plan())

    def test_the_plan_is_not_trivially_empty(self):
        """A test that passes because both sides found nothing proves nothing."""
        self.assertTrue(self._json_plan())

    def test_both_cover_every_account(self):
        plan = self._json_plan()
        self.assertEqual({"111", "222"}, {user for user, _, _ in plan})

    def test_the_interface_can_read_what_the_core_writes(self):
        """The client parses the real stream, not a fixture of one."""
        _, text = self._run_cli("apply", "--dry-run", "--json")
        records = client.parse_stream(text)
        run = client.Run(records, 0)
        self.assertIsNotNone(run.result, "stream ended without a result record")
        self.assertTrue(run.ok)
        self.assertFalse(run.blocked)
        self.assertIn("100", run.games_by_appid())

    def test_an_exclusion_survives_the_round_trip(self):
        """The excluded game reaches the interface as a change it can show."""
        _, text = self._run_cli("apply", "--dry-run", "--json")
        records = client.parse_stream(text)
        excluded = {r["appid"] for r in records
                    if r.get("kind") == client.KIND_CHANGE
                    and r.get("action") == "excluded"}
        self.assertEqual({"300"}, excluded)

    def test_dry_run_writes_nothing_in_either_mode(self):
        localconfig = self.root / "userdata" / "111" / "config" / "localconfig.vdf"
        before = localconfig.read_bytes()
        self._run_cli("apply", "--dry-run")
        self._run_cli("apply", "--dry-run", "--json")
        self.assertEqual(before, localconfig.read_bytes())

    def test_the_window_asks_for_exactly_this_command(self):
        """Guards the other half: the window must invoke `apply --dry-run`.

        If the window ever asked for something else, the equivalence proven
        above would be about a command nobody runs.
        """
        from tests.qtapp import HAVE_QT, ensure_app
        if not HAVE_QT:
            self.skipTest("PyQt6 not installed")
        ensure_app()
        from steamtrain_gui import window

        win = window.MainWindow("0.0.0", "0.0.0")
        self.addCleanup(win.deleteLater)

        seen = []
        with mock.patch.object(win.runner, "start",
                               side_effect=lambda args: seen.append(list(args)) or True):
            win.dry_run()
            win.apply_now()
        self.assertEqual([["apply", "--dry-run"], ["apply"]], seen)


if __name__ == "__main__":
    unittest.main()
