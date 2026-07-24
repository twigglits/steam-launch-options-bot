"""The window and the CLI must plan identically.

This is the executable form of the rule that the desktop interface is a client
of the Core and never a second implementation of it. If the two ever disagree
about what would be written, the safety story is gone: the window would be
showing a plan the timer will not carry out.

Compares the two rendering paths of the same planner, not the two code paths,
because there is only one planner and that is the point.
"""

import contextlib
import io
import json
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from steamtrain import cli
from steamtrain_gui import client

from tests.test_cli import fake_profile
from tests.test_steam import make_manifest, make_steam_root


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
            cfg = self.root / "userdata" / account / "config"
            cfg.mkdir(parents=True)
            (cfg / "localconfig.vdf").write_text('"UserLocalConfigStore"\n{\n}\n')
        self.state_dir = base / "state"
        self.config = base / "config.json"

        from steamtrain import rules
        data = rules.default_config()
        data["exclude"] = ["300"]
        self.config.write_text(json.dumps(data))

    def _common(self):
        return ["--steam-root", str(self.root),
                "--state-dir", str(self.state_dir),
                "--config", str(self.config)]

    def _run_cli(self, *args):
        out = io.StringIO()
        with mock.patch("steamtrain.sysinfo.detect", return_value=fake_profile("nvidia")), \
             mock.patch("steamtrain.steam.is_steam_running", return_value=False), \
             contextlib.redirect_stdout(out):
            code = cli.main([*args, *self._common()])
        return code, out.getvalue()

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
        records = [json.loads(line) for line in text.splitlines() if line.strip()]
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

    def test_dry_run_writes_nothing_in_either_mode(self):
        from steamtrain import vdf
        localconfig = self.root / "userdata" / "111" / "config" / "localconfig.vdf"
        before = localconfig.read_bytes()
        self._run_cli("apply", "--dry-run")
        self._run_cli("apply", "--dry-run", "--json")
        self.assertEqual(before, localconfig.read_bytes())
        data = vdf.loads(localconfig.read_text())
        self.assertNotIn("Software", data.get("UserLocalConfigStore", {}))

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
