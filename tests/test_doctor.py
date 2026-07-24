"""Legacy-install detection and migration (FR-8 .. FR-12, AD-10, AD-11)."""

import tempfile
import unittest
from pathlib import Path

from steamtrain import codes, doctor


class DoctorTestBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name) / "home"
        self.usr = Path(self.tmp.name) / "usr"
        self.usr.mkdir(parents=True)
        self.packaged = self.usr / "steamtrain"
        self.packaged.write_text("#!/bin/sh\n")
        self.packaged.chmod(0o755)

    def make_legacy(self, bin_=True, lib=True, units=True):
        if bin_:
            path = self.home / ".local" / "bin"
            path.mkdir(parents=True, exist_ok=True)
            (path / "steamtrain").write_text("#!/bin/sh\n")
            (path / "steamtrain").chmod(0o755)
        if lib:
            path = self.home / ".local" / "lib" / "steamtrain" / "steamtrain"
            path.mkdir(parents=True, exist_ok=True)
            (path / "__init__.py").write_text("")
        if units:
            path = self.home / ".config" / "systemd" / "user"
            path.mkdir(parents=True, exist_ok=True)
            (path / "steamtrain.service").write_text(
                "[Service]\nExecStart=%h/.local/bin/steamtrain apply\n")
            (path / "steamtrain.timer").write_text("[Timer]\n")

    def make_user_data(self):
        cfg = self.home / ".config" / "steamtrain"
        cfg.mkdir(parents=True, exist_ok=True)
        (cfg / "config.json").write_text('{"gpu_vendor": "nvidia"}\n')
        state = self.home / ".local" / "state" / "steamtrain"
        (state / "backups").mkdir(parents=True, exist_ok=True)
        (state / "state.json").write_text('{"111/100": "gamemoderun %command%"}\n')
        (state / "backups" / "localconfig-111-1.vdf").write_text("backup\n")
        return cfg / "config.json", state / "state.json"

    def legacy_path_env(self):
        return f"{self.home / '.local' / 'bin'}:{self.usr}"


class DetectionTest(DoctorTestBase):
    def test_no_findings_without_a_packaged_install(self):
        """An install.sh user who has not switched has no conflict to report."""
        self.make_legacy()
        findings = doctor.diagnose(home=self.home,
                                   path_env=self.legacy_path_env(),
                                   packaged_bin=self.usr / "does-not-exist")
        self.assertEqual([], findings)

    def test_no_findings_on_a_clean_machine(self):
        findings = doctor.diagnose(home=self.home,
                                   path_env=str(self.usr),
                                   packaged_bin=self.packaged)
        self.assertEqual([], findings)

    def test_detects_shadowing_when_legacy_bin_wins_the_path_lookup(self):
        self.make_legacy(lib=False, units=False)
        findings = doctor.diagnose(home=self.home,
                                   path_env=self.legacy_path_env(),
                                   packaged_bin=self.packaged)
        self.assertEqual(1, len(findings))
        self.assertEqual(codes.LEGACY_INSTALL_SHADOWING, findings[0].code)
        self.assertTrue(findings[0].fixable)

    def test_lone_legacy_bin_behind_usr_bin_is_inert(self):
        self.make_legacy(lib=False, units=False)
        findings = doctor.diagnose(home=self.home,
                                   path_env=f"{self.usr}:{self.home / '.local' / 'bin'}",
                                   packaged_bin=self.packaged)
        self.assertEqual([], findings)

    def test_legacy_units_are_a_conflict_regardless_of_path_order(self):
        self.make_legacy(bin_=False, lib=False)
        findings = doctor.diagnose(home=self.home,
                                   path_env=str(self.usr),
                                   packaged_bin=self.packaged)
        self.assertEqual(1, len(findings))

    def test_finding_names_every_path_found(self):
        self.make_legacy()
        findings = doctor.diagnose(home=self.home,
                                   path_env=self.legacy_path_env(),
                                   packaged_bin=self.packaged)
        paths = findings[0].paths
        self.assertTrue(any("bin/steamtrain" in p for p in paths))
        self.assertTrue(any("lib/steamtrain" in p for p in paths))
        self.assertTrue(any("steamtrain.timer" in p for p in paths))

    def test_detection_writes_nothing(self):
        self.make_legacy()
        before = sorted(p.relative_to(self.home) for p in self.home.rglob("*"))
        doctor.diagnose(home=self.home, path_env=self.legacy_path_env(),
                        packaged_bin=self.packaged)
        after = sorted(p.relative_to(self.home) for p in self.home.rglob("*"))
        self.assertEqual(before, after)

    def test_detection_survives_no_systemd_session(self):
        """No systemctl is called during detection at all, so nothing to mock."""
        self.make_legacy()
        findings = doctor.diagnose(home=self.home, path_env=self.legacy_path_env(),
                                   packaged_bin=self.packaged)
        self.assertEqual(1, len(findings))


class MigrationTest(DoctorTestBase):
    def setUp(self):
        super().setUp()
        self.calls = []

    def runner(self, argv):
        self.calls.append(argv)

    def test_removes_every_legacy_path(self):
        self.make_legacy()
        removed, failed = doctor.migrate(home=self.home, runner=self.runner)
        self.assertEqual([], failed)
        self.assertFalse((self.home / ".local" / "bin" / "steamtrain").exists())
        self.assertFalse((self.home / ".local" / "lib" / "steamtrain").exists())
        self.assertFalse((self.home / ".config" / "systemd" / "user"
                          / "steamtrain.timer").exists())
        self.assertEqual(4, len(removed))

    def test_config_and_state_survive_byte_for_byte(self):
        """FR-11. Losing state.json strands every option permanently."""
        self.make_legacy()
        config, state = self.make_user_data()
        config_before = config.read_bytes()
        state_before = state.read_bytes()
        doctor.migrate(home=self.home, runner=self.runner)
        self.assertEqual(config_before, config.read_bytes())
        self.assertEqual(state_before, state.read_bytes())
        self.assertTrue((self.home / ".local" / "state" / "steamtrain" / "backups"
                         / "localconfig-111-1.vdf").exists())

    def test_the_timer_is_disabled_before_its_units_are_removed(self):
        self.make_legacy()
        doctor.migrate(home=self.home, runner=self.runner)
        self.assertEqual(
            [["systemctl", "--user", "disable", "--now", "steamtrain.timer"]],
            self.calls)

    def test_no_systemd_session_does_not_abort_the_migration(self):
        def exploding(argv):
            raise OSError("no session bus")
        self.make_legacy()
        removed, failed = doctor.migrate(home=self.home, runner=exploding)
        self.assertEqual([], failed)
        self.assertFalse((self.home / ".local" / "bin" / "steamtrain").exists())

    def test_dry_run_removes_nothing(self):
        self.make_legacy()
        removed, failed = doctor.migrate(home=self.home, dry_run=True,
                                         runner=self.runner)
        self.assertTrue(removed)
        self.assertEqual([], self.calls, "dry run must not touch the timer either")
        self.assertTrue((self.home / ".local" / "bin" / "steamtrain").exists())

    def test_each_removal_is_reported_individually(self):
        self.make_legacy()
        removed, _ = doctor.migrate(home=self.home, runner=self.runner)
        for path, detail in removed:
            self.assertTrue(Path(path).name.startswith("steamtrain"))
            self.assertEqual("removed", detail)

    def test_unrelated_neighbouring_files_are_untouched(self):
        self.make_legacy()
        neighbour = self.home / ".local" / "bin" / "something-else"
        neighbour.write_text("keep me\n")
        units = self.home / ".config" / "systemd" / "user" / "other.service"
        units.write_text("[Service]\n")
        doctor.migrate(home=self.home, runner=self.runner)
        self.assertTrue(neighbour.exists())
        self.assertTrue(units.exists())

    def test_protected_paths_are_never_in_the_allowlist(self):
        allowed = set(doctor.removable_paths(self.home))
        for guard in doctor.protected_paths(self.home):
            self.assertNotIn(guard, allowed)
            for path in allowed:
                self.assertNotIn(guard, path.parents,
                                 f"{path} sits under protected {guard}")

    def test_migration_on_a_clean_machine_is_a_no_op(self):
        removed, failed = doctor.migrate(home=self.home, runner=self.runner)
        self.assertEqual([], removed)
        self.assertEqual([], failed)


if __name__ == "__main__":
    unittest.main()
