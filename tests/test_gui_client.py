"""The GUI's CLI client. Qt-free, so it runs headlessly in CI."""

import json
import unittest
from unittest import mock

from steamtrain_gui import client

from tests.coreproc import CORE, SKIP_REASON


class FakeCompleted:
    def __init__(self, stdout="", returncode=0, stderr=""):
        self.stdout = stdout
        self.returncode = returncode
        self.stderr = stderr


def stream(*records):
    return "".join(json.dumps(r) + "\n" for r in records)


def result(**fields):
    base = {"v": 1, "kind": "result", "ok": True, "outcome": "ok"}
    base.update(fields)
    return base


class ParseStreamTest(unittest.TestCase):
    def test_blank_lines_are_ignored(self):
        records = client.parse_stream('\n{"v": 1, "kind": "result"}\n\n')
        self.assertEqual(1, len(records))

    def test_non_json_line_is_a_protocol_error_naming_the_line(self):
        with self.assertRaises(client.ProtocolError) as caught:
            client.parse_stream('{"v": 1}\nnot json\n')
        self.assertIn("line 2", str(caught.exception))

    def test_a_newer_wire_format_is_refused_with_an_actionable_message(self):
        with self.assertRaises(client.ProtocolError) as caught:
            client.parse_stream('{"v": 2, "kind": "result"}\n')
        message = str(caught.exception)
        self.assertIn("v2", message)
        self.assertIn("mismatched", message)

    def test_unknown_kinds_pass_through_rather_than_crashing(self):
        """A newer Core may add record kinds; the client must degrade."""
        records = client.parse_stream('{"v": 1, "kind": "weather", "sunny": true}\n')
        self.assertEqual("weather", records[0]["kind"])

    def test_records_without_a_version_are_accepted(self):
        records = client.parse_stream('{"kind": "result"}\n')
        self.assertEqual(1, len(records))


class RunTest(unittest.TestCase):
    def setUp(self):
        patcher = mock.patch.object(client, "find_core", return_value="/usr/bin/steamtrain")
        patcher.start()
        self.addCleanup(patcher.stop)

    def invoke(self, text, returncode=0, stderr=""):
        return client.run(["scan"], runner=lambda argv, timeout:
                          FakeCompleted(text, returncode, stderr))

    def test_passes_json_flag_to_the_core(self):
        seen = {}

        def runner(argv, timeout):
            seen["argv"] = argv
            return FakeCompleted(stream(result()))

        client.run(["apply", "--dry-run"], runner=runner)
        self.assertEqual(["/usr/bin/steamtrain", "apply", "--dry-run", "--json"],
                         seen["argv"])

    def test_truncated_stream_is_reported_not_silently_accepted(self):
        with self.assertRaises(client.ProtocolError) as caught:
            self.invoke(stream({"v": 1, "kind": "game", "appid": "1"}))
        self.assertIn("without a result record", str(caught.exception))

    def test_empty_output_surfaces_stderr(self):
        with self.assertRaises(client.ProtocolError) as caught:
            self.invoke("", returncode=1, stderr="boom")
        self.assertIn("boom", str(caught.exception))

    def test_blocked_run_is_recognised_despite_exiting_zero(self):
        run = self.invoke(stream(result(ok=False, outcome="blocked",
                                        guardrail="steam-running",
                                        message="Steam is running")))
        self.assertEqual(0, run.returncode)
        self.assertTrue(run.blocked)
        self.assertFalse(run.ok)
        self.assertEqual("steam-running", run.guardrail)
        self.assertEqual("Steam is running", run.message)

    def test_doctor_exiting_two_is_not_treated_as_a_crash(self):
        run = client.run(["doctor"], runner=lambda argv, timeout: FakeCompleted(
            stream({"v": 1, "kind": "finding", "code": "legacy-install-shadowing"},
                   result(ok=False, outcome="error", findings=1)), 2))
        self.assertEqual(2, run.returncode)
        self.assertEqual(1, len(run.of_kind(client.KIND_FINDING)))

    def test_an_unknown_guardrail_code_is_carried_not_swallowed(self):
        """AD-3: render unknown codes generically, never crash or ignore them."""
        run = self.invoke(stream(result(ok=False, outcome="blocked",
                                        guardrail="something-new",
                                        message="a reason from a newer Core")))
        self.assertTrue(run.blocked)
        self.assertEqual("something-new", run.guardrail)
        self.assertEqual("a reason from a newer Core", run.message)


class JoiningTest(unittest.TestCase):
    """AD-15: state on change records, display metadata on game records."""

    def make(self, *records):
        return client.Run(list(records), 0)

    def test_games_are_keyed_for_joining(self):
        run = self.make({"kind": "game", "appid": "100", "name": "Fixture"},
                        {"kind": "change", "user": "1", "appid": "100",
                         "action": "set"},
                        result())
        games = run.games_by_appid()
        change = run.of_kind(client.KIND_CHANGE)[0]
        self.assertEqual("Fixture", games[change["appid"]]["name"])

    def test_a_change_without_a_game_record_is_expected_after_revert(self):
        run = self.make({"kind": "change", "user": "1", "appid": "999",
                         "action": "set"},
                        result())
        change = run.of_kind(client.KIND_CHANGE)[0]
        self.assertEqual({}, run.games_by_appid())
        # the caller falls back to the appid as the label
        self.assertEqual("999", change["appid"])


class FindCoreTest(unittest.TestCase):
    def test_missing_core_names_the_fix(self):
        with mock.patch.object(client.shutil, "which", return_value=None):
            with self.assertRaises(client.CoreNotFound) as caught:
                client.find_core()
        self.assertIn("Install the steamtrain package", str(caught.exception))

    def test_core_is_located_by_path_not_hardcoded(self):
        with mock.patch.object(client.shutil, "which",
                               return_value="/home/dev/.local/bin/steamtrain") as which:
            self.assertEqual("/home/dev/.local/bin/steamtrain", client.find_core())
        which.assert_called_once_with("steamtrain")


class VersionTest(unittest.TestCase):
    def test_parses_the_version_argparse_prints(self):
        with mock.patch.object(client, "find_core", return_value="/usr/bin/steamtrain"):
            version = client.core_version(
                runner=lambda argv, timeout: FakeCompleted("steamtrain 0.5.0\n"))
        self.assertEqual("0.5.0", version)


@unittest.skipIf(CORE is None, SKIP_REASON)
class RealCoreTest(unittest.TestCase):
    """End-to-end against the actual CLI in this checkout.

    The fakes above pin the client's behaviour; this pins the contract between
    the two halves. It is the test that fails if the Core's wire format drifts
    away from what the GUI expects, which no amount of mocking would catch.
    """

    @classmethod
    def setUpClass(cls):
        import os
        import subprocess
        import tempfile
        from pathlib import Path

        from tests.coreproc import make_bindir, make_manifest, make_steam_root, make_user

        cls.tmp = tempfile.TemporaryDirectory()
        base = Path(cls.tmp.name)

        cls.steam_root = make_steam_root(base)
        make_manifest(cls.steam_root, "100", "Fixture Game", "FixtureGame")
        make_user(cls.steam_root, "111")

        cls.bindir = make_bindir(base)
        cls.base = base
        cls.env_path = os.environ["PATH"]
        cls.subprocess = subprocess

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def run_core(self, *args):
        import os
        with mock.patch.dict(os.environ, {"PATH": self.bindir}):
            return client.run([
                *args,
                "--steam-root", str(self.steam_root),
                "--state-dir", str(self.base / "state"),
                "--config", str(self.base / "config.json"),
            ])

    def test_scan_round_trips_through_the_client(self):
        run = self.run_core("scan")
        self.assertTrue(run.ok)
        self.assertIsNotNone(run.result)
        self.assertEqual(1, len(run.of_kind(client.KIND_PROFILE)))
        games = run.games_by_appid()
        self.assertIn("100", games)
        self.assertEqual("Fixture Game", games["100"]["name"])

    def test_every_change_joins_to_a_game_record(self):
        run = self.run_core("scan")
        games = run.games_by_appid()
        for change in run.of_kind(client.KIND_CHANGE):
            self.assertIn(change["appid"], games)
            self.assertNotIn("name", change, "AD-15: no display metadata on changes")

    def test_counts_use_the_action_vocabulary_the_client_expects(self):
        run = self.run_core("scan")
        counts = run.result["counts"]
        self.assertEqual({"set", "skip-user-set", "skip-unchanged", "excluded"},
                         set(counts))

    def test_status_reports_config_absence_without_creating_it(self):
        from pathlib import Path
        config = self.base / "config-probe.json"
        import os
        with mock.patch.dict(os.environ, {"PATH": self.bindir}):
            run = client.run(["status", "--steam-root", str(self.steam_root),
                              "--state-dir", str(self.base / "state"),
                              "--config", str(config)])
        self.assertIs(run.result["config_exists"], False)
        self.assertFalse(Path(config).exists())

    def test_core_version_is_readable_for_the_parity_check(self):
        import os
        with mock.patch.dict(os.environ, {"PATH": self.bindir}):
            version = client.core_version()
        self.assertRegex(version, r"^\d+\.\d+\.\d+")


if __name__ == "__main__":
    unittest.main()
