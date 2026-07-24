"""The --json wire format (AD-2 .. AD-7, AD-15)."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from steamtrain import cli, codes, jsonio

from tests.test_cli import fake_profile
from tests.test_steam import make_manifest, make_steam_root


class JsonCliTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.root = make_steam_root(base)
        make_manifest(self.root, "100", "Fixture Game", "FixtureGame")
        make_manifest(self.root, "200", "Excluded Game", "ExcludedGame")
        for account in ("111", "222"):
            cfg = self.root / "userdata" / account / "config"
            cfg.mkdir(parents=True)
            (cfg / "localconfig.vdf").write_text('"UserLocalConfigStore"\n{\n}\n')
        self.state_dir = base / "state"
        self.config_path = base / "config.json"

    def write_config(self, **fields):
        from steamtrain import rules
        data = rules.default_config()
        data.update(fields)
        self.config_path.write_text(json.dumps(data))

    def run_json(self, *args, running=False):
        out = io.StringIO()
        argv = [
            *args, "--json",
            "--steam-root", str(self.root),
            "--state-dir", str(self.state_dir),
            "--config", str(self.config_path),
        ]
        with mock.patch("steamtrain.sysinfo.detect", return_value=fake_profile()), \
             mock.patch("steamtrain.steam.is_steam_running", return_value=running), \
             contextlib.redirect_stdout(out):
            code = cli.main(argv)
        text = out.getvalue()
        records = [json.loads(line) for line in text.splitlines() if line.strip()]
        return code, records, text

    def kinds(self, records):
        return [r["kind"] for r in records]

    def of_kind(self, records, kind):
        return [r for r in records if r["kind"] == kind]

    # --- envelope invariants (AD-2) -------------------------------------

    def test_every_command_ends_with_exactly_one_result(self):
        for command in ("scan", "status", "revert", "apply"):
            with self.subTest(command=command):
                _, records, _ = self.run_json(command)
                self.assertEqual(self.kinds(records)[-1], jsonio.KIND_RESULT)
                self.assertEqual(self.kinds(records).count(jsonio.KIND_RESULT), 1)

    def test_output_is_ndjson_not_an_array(self):
        _, _, text = self.run_json("scan")
        self.assertFalse(text.lstrip().startswith("["))
        for line in text.splitlines():
            json.loads(line)

    def test_every_record_is_versioned_and_of_a_known_kind(self):
        _, records, _ = self.run_json("scan")
        for record in records:
            self.assertEqual(record["v"], jsonio.VERSION)
            self.assertIn(record["kind"], jsonio.KINDS)

    # --- change identity and metadata split (AD-4, AD-15) ----------------

    def test_change_records_are_per_user_per_appid(self):
        _, records, _ = self.run_json("scan")
        changes = self.of_kind(records, jsonio.KIND_CHANGE)
        pairs = {(c["user"], c["appid"]) for c in changes}
        self.assertEqual(len(pairs), len(changes), "duplicate (user, appid)")
        self.assertEqual({"111", "222"}, {c["user"] for c in changes},
                         "both Steam accounts must be represented")

    def test_change_records_carry_no_display_metadata(self):
        """AD-15: plan_revert sets name=appid, so names must not ride on changes."""
        _, records, _ = self.run_json("scan")
        for change in self.of_kind(records, jsonio.KIND_CHANGE):
            self.assertNotIn("name", change)
            self.assertNotIn("runtime", change)

    def test_game_records_carry_the_display_metadata(self):
        _, records, _ = self.run_json("scan")
        games = self.of_kind(records, jsonio.KIND_GAME)
        self.assertEqual({"100", "200"}, {g["appid"] for g in games})
        by_appid = {g["appid"]: g for g in games}
        self.assertEqual(by_appid["100"]["name"], "Fixture Game")
        self.assertIn("runtime", by_appid["100"])

    def test_game_records_precede_the_changes_that_reference_them(self):
        _, records, _ = self.run_json("scan")
        kinds = self.kinds(records)
        last_game = max(i for i, k in enumerate(kinds) if k == jsonio.KIND_GAME)
        first_change = min(i for i, k in enumerate(kinds) if k == jsonio.KIND_CHANGE)
        self.assertLess(last_game, first_change)

    # --- exclusions are visible (AD-5) -----------------------------------

    def test_excluded_games_are_emitted_not_dropped(self):
        self.write_config(exclude=["200"])
        _, records, _ = self.run_json("scan")
        excluded = [c for c in self.of_kind(records, jsonio.KIND_CHANGE)
                    if c["action"] == codes.EXCLUDED]
        self.assertEqual({"200"}, {c["appid"] for c in excluded})
        self.assertEqual({"111", "222"}, {c["user"] for c in excluded})

    def test_excluded_games_are_still_never_written(self):
        self.write_config(exclude=["200"])
        self.run_json("apply")
        from steamtrain import vdf
        data = vdf.loads((self.root / "userdata" / "111" / "config"
                          / "localconfig.vdf").read_text())
        apps = data["UserLocalConfigStore"]["Software"]["Valve"]["Steam"]["apps"]
        self.assertNotIn("200", apps)

    # --- counts (AD-2 + AD-3) --------------------------------------------

    def test_counts_carry_every_action_key_including_zeros(self):
        _, records, _ = self.run_json("scan")
        counts = records[-1]["counts"]
        self.assertEqual(set(counts), set(codes.ACTIONS))

    def test_counts_sum_to_the_number_of_change_records(self):
        self.write_config(exclude=["200"])
        _, records, _ = self.run_json("scan")
        counts = records[-1]["counts"]
        self.assertEqual(sum(counts.values()),
                         len(self.of_kind(records, jsonio.KIND_CHANGE)))

    # --- progress (AD-2) --------------------------------------------------

    def test_progress_total_matches_the_change_record_count(self):
        _, records, _ = self.run_json("scan")
        progress = self.of_kind(records, jsonio.KIND_PROGRESS)
        self.assertTrue(progress)
        self.assertEqual(progress[-1]["total"],
                         len(self.of_kind(records, jsonio.KIND_CHANGE)))
        self.assertEqual(progress[-1]["done"], progress[-1]["total"])

    # --- blocked is not failed (AD-7) -------------------------------------

    def test_apply_blocked_by_steam_exits_zero_with_a_guardrail_code(self):
        code, records, _ = self.run_json("apply", running=True)
        result = records[-1]
        self.assertEqual(code, 0, "the timer must not record a failure")
        self.assertIs(result["ok"], False)
        self.assertEqual(result["outcome"], codes.BLOCKED)
        self.assertEqual(result["guardrail"], codes.STEAM_RUNNING)
        self.assertEqual(result["written"], 0)

    def test_apply_dry_run_is_not_blocked_while_steam_runs(self):
        code, records, _ = self.run_json("apply", "--dry-run", running=True)
        self.assertEqual(code, 0)
        self.assertIs(records[-1]["ok"], True)
        self.assertIs(records[-1]["dry_run"], True)

    def test_apply_writes_and_reports_the_written_count(self):
        code, records, _ = self.run_json("apply")
        self.assertEqual(code, 0)
        self.assertEqual(records[-1]["outcome"], codes.OK)
        self.assertGreater(records[-1]["written"], 0)

    def test_missing_steam_root_reports_a_guardrail_not_a_bare_message(self):
        out = io.StringIO()
        with mock.patch("steamtrain.steam.find_steam_root", return_value=None), \
             contextlib.redirect_stdout(out):
            code = cli.main(["scan", "--json", "--config", str(self.config_path)])
        record = json.loads(out.getvalue().splitlines()[-1])
        self.assertEqual(code, 1)
        self.assertEqual(record["guardrail"], codes.NO_STEAM_ROOT)

    def test_invalid_config_reports_a_guardrail(self):
        self.config_path.write_text("{ not json")
        code, records, _ = self.run_json("scan")
        self.assertEqual(code, 1)
        self.assertEqual(records[-1]["guardrail"], codes.CONFIG_INVALID)

    # --- status probes config without creating it (AD-6) ------------------

    def test_status_reports_config_absent_and_does_not_create_it(self):
        self.assertFalse(self.config_path.exists())
        _, records, _ = self.run_json("status")
        self.assertIs(records[-1]["config_exists"], False)
        self.assertFalse(self.config_path.exists(),
                         "status must not be what stops the first-run screen appearing")

    def test_status_reports_config_present_when_it_is(self):
        self.write_config()
        _, records, _ = self.run_json("status")
        self.assertIs(records[-1]["config_exists"], True)

    def test_status_reports_managed_options(self):
        self.run_json("apply")
        _, records, _ = self.run_json("status")
        self.assertTrue(records[-1]["managed"])

    # --- revert (AD-15 allows changes with no game record) ----------------

    def test_revert_emits_changes_and_counts(self):
        self.run_json("apply")
        _, records, _ = self.run_json("revert")
        changes = self.of_kind(records, jsonio.KIND_CHANGE)
        self.assertTrue(changes)
        self.assertTrue(all(c["proposed"] == "" for c in changes))
        self.assertEqual(set(records[-1]["counts"]), set(codes.ACTIONS))

    def test_revert_emits_no_game_records(self):
        self.run_json("apply")
        _, records, _ = self.run_json("revert")
        self.assertEqual([], self.of_kind(records, jsonio.KIND_GAME))


class TextModeUnchangedTest(unittest.TestCase):
    """Without --json nothing about the existing output may change."""

    def test_no_json_flag_means_no_json_on_stdout(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        base = Path(tmp.name)
        root = make_steam_root(base)
        make_manifest(root, "100", "Fixture Game", "FixtureGame")
        cfg = root / "userdata" / "111" / "config"
        cfg.mkdir(parents=True)
        (cfg / "localconfig.vdf").write_text('"UserLocalConfigStore"\n{\n}\n')
        out = io.StringIO()
        with mock.patch("steamtrain.sysinfo.detect", return_value=fake_profile()), \
             contextlib.redirect_stdout(out):
            cli.main(["scan", "--steam-root", str(root),
                      "--state-dir", str(base / "state"),
                      "--config", str(base / "config.json")])
        self.assertIn("Fixture Game", out.getvalue())
        for line in out.getvalue().splitlines():
            self.assertFalse(line.startswith("{"), "text mode leaked a JSON record")


if __name__ == "__main__":
    unittest.main()
