"""The settings window: the only surface steamtrain has, and the only thing
that can run it.

There is no tray icon and no background presence. Scheduled runs are a timer
inside this process, so "is steamtrain running?" has one answer a person can
check without a terminal: the window is open, or it is not. Close it and
nothing of steamtrain is left running.
"""

from PyQt6.QtCore import Qt, QTimer, pyqtSignal
from PyQt6.QtGui import QAction, QKeySequence
from PyQt6.QtWidgets import (
    QDialog, QDialogButtonBox, QFormLayout, QFrame, QGroupBox, QHBoxLayout,
    QHeaderView, QLabel, QMainWindow, QMessageBox, QProgressBar, QPushButton,
    QRadioButton, QTableView, QVBoxLayout, QWidget,
)

from . import client, models
from .qtclient import CoreRunner

# Half an hour, the interval the systemd timer this replaces used: long enough
# that a scheduled run costs nothing noticeable, short enough to catch a game
# installed while you were playing another one.
SCHEDULE_INTERVAL_MS = 30 * 60 * 1000

VENDOR_LABELS = [
    ("auto", "Detect automatically"),
    ("nvidia", "NVIDIA"),
    ("amd", "AMD"),
    ("intel", "Intel"),
]


class Banner(QLabel):
    """A plain-language explanation of why something is blocked.

    Carries an icon and words, never colour alone - the state has to survive
    being read by someone who cannot distinguish the shade.
    """

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWordWrap(True)
        self.setFrameShape(QFrame.Shape.StyledPanel)
        self.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        self.setContentsMargins(8, 8, 8, 8)
        self.hide()

    def show_message(self, text, severity="info"):
        prefix = {"info": "ℹ", "warning": "⚠", "error": "✖"}.get(severity, "ℹ")
        self.setText(f"{prefix}  {text}")
        self.setAccessibleName(f"{severity}: {text}")
        self.show()

    def clear_message(self):
        self.clear()
        self.hide()


class FirstRunDialog(QDialog):
    """Confirm what steamtrain detected, or correct the GPU vendor.

    Shown only when no config file exists yet, which the Core reports as
    `config_exists` rather than the interface guessing from a path.
    """

    def __init__(self, profile, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Welcome to steamtrain")
        self._profile = profile or {}
        self._buttons = []

        layout = QVBoxLayout(self)
        intro = QLabel(
            "steamtrain sets Steam launch options to suit <b>this</b> machine. "
            "Here is what it detected — correct the GPU if it got it wrong.")
        intro.setWordWrap(True)
        layout.addWidget(intro)

        detected = QGroupBox("Detected")
        form = QFormLayout(detected)
        gpu = self._profile.get("gpu_name") or "unknown"
        driver = self._profile.get("gpu_driver") or ""
        form.addRow("System:", QLabel(self._profile.get("distro", "unknown")))
        form.addRow("Session:", QLabel(
            f"{self._profile.get('desktop', 'unknown')} "
            f"({self._profile.get('session', 'unknown')})"))
        form.addRow("Graphics:", QLabel(
            f"{gpu} [{self._profile.get('gpu_vendor', 'unknown')}]"
            + (f" {driver}" if driver else "")))
        form.addRow("Helpers:", QLabel(
            f"gamemode: {'yes' if self._profile.get('has_gamemode') else 'no'}   "
            f"MangoHud: {'yes' if self._profile.get('has_mangohud') else 'no'}"))
        layout.addWidget(detected)

        choice = QGroupBox("Graphics vendor")
        choice_layout = QVBoxLayout(choice)
        detected_vendor = self._profile.get("gpu_vendor", "unknown")
        for value, label in VENDOR_LABELS:
            button = QRadioButton(label)
            button.setProperty("vendor", value)
            if value == "auto":
                button.setChecked(True)
                if detected_vendor != "unknown":
                    button.setText(f"{label} — currently {detected_vendor}")
            choice_layout.addWidget(button)
            self._buttons.append(button)
        if detected_vendor == "unknown":
            warn = QLabel("Autodetection failed. Pick the GPU that drives your games.")
            warn.setWordWrap(True)
            choice_layout.addWidget(warn)
        layout.addWidget(choice)

        note = QLabel(
            "Nothing is written to Steam yet. You choose when to apply, and "
            "steamtrain never overwrites options you set yourself.")
        note.setWordWrap(True)
        layout.addWidget(note)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

    def chosen_vendor(self):
        for button in self._buttons:
            if button.isChecked():
                return button.property("vendor")
        return "auto"


class MigrationDialog(QDialog):
    """Blocking explanation of a legacy install, with a one-click repair.

    Lists every path before removing anything: a dialog that deletes files it
    did not name is not consent.
    """

    def __init__(self, finding, parent=None):
        super().__init__(parent)
        self.setWindowTitle("An old steamtrain install is in the way")
        self._finding = finding

        layout = QVBoxLayout(self)
        explain = QLabel(
            "<b>The steamtrain you installed is not the one running.</b><br><br>"
            "An older install under your home directory takes precedence over "
            "the packaged one, so the package cannot do anything until it is "
            "removed. This is silent otherwise — nothing would have told you.")
        explain.setWordWrap(True)
        layout.addWidget(explain)

        layout.addWidget(QLabel("These will be removed:"))
        for path in finding.get("paths", []):
            item = QLabel(f"    {path}")
            item.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
            layout.addWidget(item)

        keep = QLabel(
            "Your settings and history are <b>not</b> touched: "
            "<code>~/.config/steamtrain</code> and "
            "<code>~/.local/state/steamtrain</code> are left exactly as they are, "
            "so options steamtrain already set stay revertible.")
        keep.setWordWrap(True)
        layout.addWidget(keep)

        buttons = QDialogButtonBox()
        self.migrate_button = buttons.addButton(
            "Remove old install", QDialogButtonBox.ButtonRole.AcceptRole)
        buttons.addButton("Not now", QDialogButtonBox.ButtonRole.RejectRole)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)


class MainWindow(QMainWindow):
    quitRequested = pyqtSignal()

    def __init__(self, gui_version, core_version, parent=None):
        super().__init__(parent)
        self.setWindowTitle("steamtrain")
        self.resize(1000, 640)

        self._gui_version = gui_version
        self._core_version = core_version
        self._degraded = False
        self._last_run = None

        self.runner = CoreRunner(self)
        self.runner.finished.connect(self._on_run_finished)
        self.runner.failed.connect(self._on_run_failed)
        self.runner.record.connect(self._on_record)
        self.runner.busyChanged.connect(self._on_busy_changed)

        # Scheduled runs are this timer and nothing else, and it runs for as
        # long as the window does. There is no switch because opening the
        # window is the switch: one fact, visible on screen, with no stored
        # preference that could disagree with it.
        self.schedule = QTimer(self)
        self.schedule.setInterval(SCHEDULE_INTERVAL_MS)
        self.schedule.timeout.connect(self._scheduled_run)
        self.schedule.start()

        self._build()
        self._update_schedule_row()
        # Deferred rather than called here: starting a worker before the event
        # loop exists races with teardown if the process never reaches exec().
        QTimer.singleShot(0, self.refresh)

    # ---------------------------------------------------------------- build

    def _build(self):
        central = QWidget()
        self.setCentralWidget(central)
        layout = QVBoxLayout(central)

        self.banner = Banner()
        layout.addWidget(self.banner)

        layout.addWidget(self._build_status())
        layout.addLayout(self._build_actions())

        self.progress = QProgressBar()
        self.progress.setTextVisible(True)
        self.progress.hide()
        layout.addWidget(self.progress)

        self.model = models.GameTableModel(self)
        self.table = QTableView()
        self.table.setModel(self.model)
        self.table.setSelectionBehavior(QTableView.SelectionBehavior.SelectRows)
        self.table.setEditTriggers(QTableView.EditTrigger.NoEditTriggers)
        self.table.setAlternatingRowColors(True)
        self.table.setSortingEnabled(False)
        self.table.horizontalHeader().setSectionResizeMode(
            models.COL_NAME, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(
            models.COL_CURRENT, QHeaderView.ResizeMode.Stretch)
        self.table.horizontalHeader().setSectionResizeMode(
            models.COL_PROPOSED, QHeaderView.ResizeMode.Stretch)
        # Status is short and load-bearing: it must never be the column that
        # gets truncated, because it is what tells the user whether their own
        # hand-set value is being preserved.
        self.table.horizontalHeader().setSectionResizeMode(
            models.COL_STATUS, QHeaderView.ResizeMode.ResizeToContents)
        layout.addWidget(self.table, stretch=1)
        self._build_shortcuts()

    def _build_status(self):
        box = QGroupBox("Status")
        form = QFormLayout(box)

        self.schedule_label = QLabel("")
        self.schedule_label.setWordWrap(True)
        self.schedule_label.setToolTip(
            "Newly installed games get their launch options every 30 minutes "
            "while this window is open. Close it and nothing of steamtrain "
            "runs.")
        form.addRow("Scheduled runs:", self.schedule_label)

        self.steam_label = QLabel("—")
        form.addRow("Steam:", self.steam_label)

        self.managed_label = QLabel("—")
        form.addRow("Managed options:", self.managed_label)

        core = self._core_version or "unknown"
        self.version_label = QLabel(f"interface {self._gui_version}, core {core}")
        self.version_label.setToolTip(
            "Packages installed from a release file do not update themselves. "
            "Quote these versions when reporting a problem.")
        form.addRow("Version:", self.version_label)
        return box

    def _build_actions(self):
        row = QHBoxLayout()
        self.dry_run_button = QPushButton("Dry run")
        self.dry_run_button.setToolTip("Show what would change. Writes nothing.")
        self.dry_run_button.clicked.connect(self.dry_run)

        self.apply_button = QPushButton("Apply now")
        self.apply_button.setToolTip("Write the proposed launch options.")
        self.apply_button.clicked.connect(self.apply_now)

        self.revert_button = QPushButton("Revert")
        self.revert_button.setToolTip(
            "Clear every option steamtrain set, back to empty.")
        self.revert_button.clicked.connect(self.revert)

        self.refresh_button = QPushButton("Refresh")
        self.refresh_button.clicked.connect(self.refresh)

        row.addWidget(self.dry_run_button)
        row.addWidget(self.apply_button)
        row.addWidget(self.revert_button)
        row.addStretch(1)
        row.addWidget(self.refresh_button)
        return row

    def _build_shortcuts(self):
        for text, sequence, slot in (
            ("&Refresh", QKeySequence.StandardKey.Refresh, self.refresh),
            ("&Apply now", QKeySequence("Ctrl+Return"), self.apply_now),
            ("&Dry run", QKeySequence("Ctrl+D"), self.dry_run),
            ("&Quit", QKeySequence.StandardKey.Quit, self.quitRequested.emit),
        ):
            action = QAction(text, self)
            action.setShortcut(sequence)
            action.triggered.connect(slot)
            self.addAction(action)

    # -------------------------------------------------------------- actions

    def refresh(self):
        self._start(["scan"], "Reading your library…")

    def dry_run(self):
        self._start(["apply", "--dry-run"], "Working out what would change…")

    def apply_now(self):
        self._start(["apply"], "Writing launch options…")

    def revert(self):
        managed = len(self._managed())
        confirm = QMessageBox(self)
        confirm.setWindowTitle("Revert launch options")
        confirm.setIcon(QMessageBox.Icon.Question)
        confirm.setText(
            f"Clear the launch options steamtrain set for {managed} "
            f"{'game' if managed == 1 else 'games'}?")
        confirm.setInformativeText(
            "Options you set yourself are not touched. steamtrain can set "
            "them again afterwards.")
        confirm.setStandardButtons(
            QMessageBox.StandardButton.Cancel | QMessageBox.StandardButton.Yes)
        confirm.setDefaultButton(QMessageBox.StandardButton.Cancel)
        if confirm.exec() == QMessageBox.StandardButton.Yes:
            self._start(["revert"], "Reverting…")

    def _start(self, args, busy_text):
        if self._degraded:
            return
        self.progress.setRange(0, 0)
        self.progress.setFormat(busy_text)
        self.progress.show()
        if not self.runner.start(args):
            self.progress.hide()

    def _managed(self):
        if self._last_run is None:
            return {}
        result = self._last_run.result or {}
        return result.get("managed", {}) or {}

    # --------------------------------------------------------------- events

    def _on_record(self, record):
        if record.get("kind") == client.KIND_PROGRESS:
            total = record.get("total") or 0
            done = record.get("done") or 0
            if total:
                self.progress.setRange(0, total)
                self.progress.setValue(done)
                self.progress.setFormat(f"%v of %m (%p%)")

    def _on_busy_changed(self, busy):
        for button in (self.dry_run_button, self.apply_button,
                       self.revert_button, self.refresh_button):
            button.setEnabled(not busy and not self._degraded)
        if not busy:
            self.progress.hide()
        self._apply_guardrail_state()

    def _on_run_finished(self, run):
        self._last_run = run
        self.model.set_rows(models.rows_from_run(run))
        self.table.setColumnHidden(models.COL_ACCOUNT, not self.model.multi_account)

        result = run.result or {}
        if run.blocked:
            self.banner.show_message(
                run.message or "steamtrain declined to write.", "warning")
        elif not run.ok:
            self.banner.show_message(
                run.message or "steamtrain reported a problem.", "error")
        else:
            self.banner.clear_message()

        if "managed" in result:
            count = len(result["managed"])
            self.managed_label.setText(
                f"{count} {'option' if count == 1 else 'options'}")
        elif "counts" in result:
            counts = result["counts"]
            self.managed_label.setText(
                ", ".join(f"{models.STATUS_TEXT.get(k, k).lower()}: {v}"
                          for k, v in counts.items() if v))

        if "steam_running" in result:
            self._set_steam_running(bool(result["steam_running"]))

    def _on_run_failed(self, message):
        self.banner.show_message(message, "error")

    def _set_steam_running(self, running):
        self.steam_label.setText("running" if running else "not running")
        self._steam_running = running
        self._apply_guardrail_state()

    def _apply_guardrail_state(self):
        running = getattr(self, "_steam_running", False)
        if running and not self.runner.busy:
            self.apply_button.setEnabled(False)
            self.revert_button.setEnabled(False)
            self.apply_button.setToolTip(
                "Steam is running. It rewrites its config when it exits, which "
                "would discard anything written now. Close Steam to enable this.")
            if not self.banner.isVisible():
                self.banner.show_message(
                    "Steam is running, so launch options cannot be written yet. "
                    "Close Steam and try again — the scheduled run retries "
                    "automatically.", "info")
        elif not self.runner.busy and not self._degraded:
            self.apply_button.setEnabled(True)
            self.revert_button.setEnabled(True)
            self.apply_button.setToolTip("Write the proposed launch options.")

    # ------------------------------------------------------------- scheduling

    def _update_schedule_row(self):
        # Says what is happening, not what was configured: this row is read
        # back from the timer, so it cannot claim a run that is not coming.
        text = ("every 30 minutes, for as long as this window stays open"
                if self.schedule.isActive() else
                "stopped — press Apply when you want a run")
        self.schedule_label.setText(text)
        self.schedule_label.setAccessibleName(f"Scheduled runs: {text}")

    def _scheduled_run(self):
        """Apply, unless the user is already driving the Core by hand.

        Skipped rather than queued: the next tick is half an hour away and
        applying twice would only repeat work that is idempotent anyway.
        """
        if self.runner.busy or self._degraded:
            return
        self.apply_now()

    # ------------------------------------------------------------- degraded

    def closeEvent(self, event):
        # Closing is the off switch for scheduled runs, so stop the timer here
        # rather than relying on teardown order.
        self.schedule.stop()
        self.runner.wait()
        super().closeEvent(event)

    def set_degraded(self, reason):
        """Read-only mode after a declined migration."""
        self._degraded = True
        self.banner.show_message(reason, "warning")
        for button in (self.dry_run_button, self.apply_button,
                       self.revert_button, self.refresh_button):
            button.setEnabled(False)
        self.schedule.stop()
        self._update_schedule_row()
