"""`steamtrain doctor` exit codes and the FR-9 warning on every subcommand."""

import contextlib
import io
import json
import unittest
from unittest import mock

from steamtrain import cli, codes, doctor


def finding():
    return doctor.Finding(
        code=codes.LEGACY_INSTALL_SHADOWING,
        message="an old user-level install is shadowing the packaged one",
        paths=["/home/x/.local/bin/steamtrain"],
        fixable=True,
    )


class DoctorCliTest(unittest.TestCase):
    def run_cli(self, *args, findings=(), migrate=None):
        out, err = io.StringIO(), io.StringIO()
        patches = [mock.patch("steamtrain.doctor.diagnose",
                              return_value=list(findings))]
        if migrate is not None:
            patches.append(mock.patch("steamtrain.doctor.migrate",
                                      return_value=migrate))
        with contextlib.ExitStack() as stack:
            for patch in patches:
                stack.enter_context(patch)
            stack.enter_context(contextlib.redirect_stdout(out))
            stack.enter_context(contextlib.redirect_stderr(err))
            code = cli.main(list(args))
        return code, out.getvalue(), err.getvalue()

    def records(self, text):
        return [json.loads(line) for line in text.splitlines() if line.strip()]

    def test_clean_machine_exits_zero(self):
        code, out, _ = self.run_cli("doctor")
        self.assertEqual(0, code)
        self.assertIn("No problems found", out)

    def test_unfixed_findings_exit_two_so_scripts_can_branch(self):
        code, out, _ = self.run_cli("doctor", findings=[finding()])
        self.assertEqual(2, code)
        self.assertIn("doctor --fix", out)

    def test_findings_are_emitted_as_records_in_json_mode(self):
        code, out, _ = self.run_cli("doctor", "--json", findings=[finding()])
        records = self.records(out)
        self.assertEqual(2, code)
        self.assertEqual("finding", records[0]["kind"])
        self.assertEqual(codes.LEGACY_INSTALL_SHADOWING, records[0]["code"])
        self.assertEqual("result", records[-1]["kind"])
        self.assertIs(records[-1]["ok"], False)

    def test_successful_fix_exits_zero(self):
        code, out, _ = self.run_cli(
            "doctor", "--fix", findings=[finding()],
            migrate=([("/home/x/.local/bin/steamtrain", "removed")], []))
        self.assertEqual(0, code)
        self.assertIn("Migrated", out)
        self.assertIn("untouched", out)

    def test_dry_run_fix_exits_two_because_nothing_was_fixed(self):
        code, out, _ = self.run_cli(
            "doctor", "--fix", "--dry-run", findings=[finding()],
            migrate=([("/home/x/.local/bin/steamtrain", "would remove")], []))
        self.assertEqual(2, code)
        self.assertIn("nothing was removed", out)

    def test_partial_failure_exits_two_and_names_what_remains(self):
        code, _, err = self.run_cli(
            "doctor", "--fix", findings=[finding()],
            migrate=([("/a", "removed")], [("/b", "Permission denied")]))
        self.assertEqual(2, code)
        self.assertIn("/b", err)
        self.assertIn("still need removing", err)

    def test_json_fix_reports_removed_and_failed_separately(self):
        code, out, _ = self.run_cli(
            "doctor", "--fix", "--json", findings=[finding()],
            migrate=([("/a", "removed")], [("/b", "Permission denied")]))
        result = self.records(out)[-1]
        self.assertEqual(2, code)
        self.assertEqual([{"path": "/a", "detail": "removed"}], result["removed"])
        self.assertEqual([{"path": "/b", "error": "Permission denied"}],
                         result["failed"])


class LegacyWarningTest(unittest.TestCase):
    """FR-9: every subcommand warns, and none of them refuses to run."""

    def run_scan(self, findings, json_mode=False):
        out, err = io.StringIO(), io.StringIO()
        argv = ["scan"] + (["--json"] if json_mode else [])
        with mock.patch("steamtrain.doctor.diagnose", return_value=list(findings)), \
             mock.patch("steamtrain.steam.find_steam_root", return_value=None), \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = cli.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_warning_names_the_conflict_and_the_fix(self):
        _, _, err = self.run_scan([finding()])
        self.assertIn("shadowing", err)
        self.assertIn("/home/x/.local/bin/steamtrain", err)
        self.assertIn("not the code being run", err)
        self.assertIn("steamtrain doctor --fix", err)

    def test_warning_goes_to_stderr_never_stdout(self):
        _, out, _ = self.run_scan([finding()])
        self.assertNotIn("shadowing", out)

    def test_warning_does_not_prevent_the_command_running(self):
        code, _, err = self.run_scan([finding()])
        # scan still ran and reported its own no-steam-root failure
        self.assertEqual(1, code)
        self.assertIn("no Steam installation found", err)

    def test_json_mode_emits_a_finding_record_before_the_command_output(self):
        _, out, _ = self.run_scan([finding()], json_mode=True)
        records = [json.loads(line) for line in out.splitlines() if line.strip()]
        self.assertEqual("finding", records[0]["kind"])
        self.assertEqual("result", records[-1]["kind"])

    def test_clean_machine_prints_no_warning(self):
        _, _, err = self.run_scan([])
        self.assertNotIn("PROBLEM", err)

    def test_doctor_itself_does_not_double_report(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch("steamtrain.doctor.diagnose",
                        return_value=[finding()]) as diagnose, \
             contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            cli.main(["doctor"])
        self.assertEqual(1, diagnose.call_count,
                         "doctor must not run the FR-9 warning as well")


if __name__ == "__main__":
    unittest.main()
