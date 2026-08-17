"""Startup sequence and wiring.

The order of the startup checks matters and is not arbitrary. A legacy
~/.local install makes the Core report an *old version*, so checking version
parity first would show "version mismatch" — a dead end — to precisely the
user whose real problem is the shadowing, and the dialog that fixes it would
never open. Doctor findings are therefore evaluated before parity.
"""

import sys
from pathlib import Path

from PyQt6.QtCore import QSettings
from PyQt6.QtGui import QIcon
from PyQt6.QtWidgets import QApplication, QDialog, QMessageBox

from . import __version__, client, system
from .window import FirstRunDialog, MainWindow, MigrationDialog

ICON_NAME = "steamtrain"
LEGACY_SHADOWING = "legacy-install-shadowing"
LEGACY_TIMER_KEY = "legacy_timer_disabled"


def load_icon():
    """Themed icon when installed, the shipped file when run from a checkout."""
    icon = QIcon.fromTheme(ICON_NAME)
    if not icon.isNull():
        return icon
    local = Path(__file__).resolve().parent.parent / "packaging" / "icons" / "steamtrain.svg"
    return QIcon(str(local)) if local.is_file() else QIcon()


def _fatal(message, detail=""):
    box = QMessageBox()
    box.setIcon(QMessageBox.Icon.Critical)
    box.setWindowTitle("steamtrain")
    box.setText(message)
    if detail:
        box.setInformativeText(detail)
    box.exec()
    return 1


def migrate_legacy_timer(settings=None):
    """Switch off the systemd timer older releases installed. Once, ever.

    That timer would keep applying options with this window shut, which is the
    one thing this design promises cannot happen. Recorded as done afterwards
    so a unit the user later writes under the same name is left alone.
    """
    settings = settings or QSettings()
    if settings.value(LEGACY_TIMER_KEY, False, type=bool):
        return False
    system.disable_legacy_timer()
    settings.setValue(LEGACY_TIMER_KEY, True)
    return True


def check_core():
    """(path, error). Absent Core is fatal — everything runs through it."""
    try:
        return client.find_core(), None
    except client.CoreNotFound as exc:
        return None, str(exc)


def doctor_findings():
    """Findings, or None when doctor could not be consulted at all."""
    try:
        run = client.run(["doctor"])
    except (client.CoreNotFound, client.ProtocolError, OSError):
        return None
    return run.of_kind(client.KIND_FINDING)


def shadowing_finding(findings):
    for finding in findings or ():
        if finding.get("code") == LEGACY_SHADOWING:
            return finding
    return None


def run_migration(finding, parent=None):
    """Show the blocking dialog and, if accepted, repair. Returns True if fixed."""
    dialog = MigrationDialog(finding, parent)
    if dialog.exec() != QDialog.DialogCode.Accepted:
        return False
    try:
        run = client.run(["doctor", "--fix"])
    except (client.ProtocolError, OSError) as exc:
        QMessageBox.critical(parent, "steamtrain",
                             f"Could not remove the old install: {exc}")
        return False
    result = run.result or {}
    if result.get("failed"):
        detail = "\n".join(f"{item['path']}: {item['error']}"
                           for item in result["failed"])
        QMessageBox.warning(
            parent, "steamtrain",
            "Some files could not be removed. Remove them by hand, then "
            "restart steamtrain:\n\n" + detail)
        return False
    return True


def check_version_parity(gui_version, parent=None):
    """(core_version, ok). Only meaningful once shadowing is ruled out."""
    try:
        core = client.core_version()
    except (client.CoreNotFound, client.ProtocolError, OSError):
        return None, True  # unreadable version is not worth blocking on
    if core != gui_version:
        QMessageBox.critical(
            parent, "steamtrain",
            "The two steamtrain packages do not match.\n\n"
            f"Interface: {gui_version}\nCore: {core}\n\n"
            "They speak a versioned protocol to each other, so install "
            "matching versions of steamtrain and steamtrain-gui.")
        return core, False
    return core, True


def maybe_first_run(parent=None):
    """Show the welcome screen only when the Core says no config exists."""
    try:
        status = client.run(["status"])
    except (client.ProtocolError, OSError):
        return
    if (status.result or {}).get("config_exists", True):
        return
    try:
        scan = client.run(["scan"])
    except (client.ProtocolError, OSError):
        return
    profiles = scan.of_kind(client.KIND_PROFILE)
    dialog = FirstRunDialog(profiles[0] if profiles else {}, parent)
    if dialog.exec() != QDialog.DialogCode.Accepted:
        return
    try:
        client.run(["setup", "--gpu-vendor", dialog.chosen_vendor()])
    except (client.ProtocolError, OSError) as exc:
        QMessageBox.warning(parent, "steamtrain",
                            f"Could not save that setting: {exc}")


def main(argv=None):
    argv = list(sys.argv if argv is None else argv)
    app = QApplication(argv)
    app.setApplicationName("steamtrain")
    # Names the QSettings file (~/.config/steamtrain/steamtrain.conf), which is
    # where the one-shot legacy-timer migration below records that it ran.
    app.setOrganizationName("steamtrain")
    app.setDesktopFileName("steamtrain")
    app.setWindowIcon(load_icon())

    migrate_legacy_timer()

    _, error = check_core()
    if error:
        return _fatal("steamtrain's command-line component is missing.", error)

    # 1. Shadowing first: it is the cause of the version mismatch below.
    finding = shadowing_finding(doctor_findings())
    degraded_reason = None
    if finding is not None:
        if not run_migration(finding):
            degraded_reason = (
                "An old steamtrain install under your home directory is still "
                "taking precedence, so nothing here would take effect. Remove "
                "it, or run `steamtrain doctor --fix`, then reopen this window.")

    # 2. Version parity, now that a stale shadowed Core has been ruled out.
    core_version = None
    if degraded_reason is None:
        core_version, ok = check_version_parity(__version__)
        if not ok:
            return 1
        maybe_first_run()

    window = MainWindow(__version__, core_version)
    if degraded_reason:
        window.set_degraded(degraded_reason)

    # Closing the window quits, and quitting is the whole story: nothing of
    # steamtrain outlives this process, scheduled runs included.
    window.quitRequested.connect(app.quit)
    window.show()

    app.aboutToQuit.connect(window.runner.wait)
    return app.exec()
