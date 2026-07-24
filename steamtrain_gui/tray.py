"""Optional system-tray presence.

Strictly a convenience layer. Everything reachable here is also in the settings
window, because stock GNOME shows no tray at all and those users must lose
nothing. If there is no tray host, no icon is created and the window simply
becomes the only surface.
"""

from PyQt6.QtCore import QRectF, Qt, pyqtSignal
from PyQt6.QtGui import QColor, QIcon, QPainter, QPen, QPixmap
from PyQt6.QtWidgets import QMenu, QSystemTrayIcon

STATE_HEALTHY = "healthy"
STATE_ATTENTION = "attention"
STATE_BLOCKED = "blocked"

TOOLTIPS = {
    STATE_HEALTHY: "steamtrain: launch options are up to date.",
    STATE_ATTENTION: "steamtrain: something needs your attention.",
    STATE_BLOCKED: "steamtrain: waiting for Steam to close before writing.",
}


def tray_available():
    """The single place this question is asked.

    Two call sites would eventually disagree - the window offering an
    autostart checkbox while the tray never appears, for instance - so every
    caller routes through here.
    """
    return QSystemTrayIcon.isSystemTrayAvailable()


def _badge(base_icon, state, size=64):
    """Base icon plus a badge whose SHAPE, not only colour, carries the state."""
    pixmap = base_icon.pixmap(size, size)
    if pixmap.isNull():
        # No themed icon and no shipped file. Fall back to a blank canvas
        # rather than returning the null icon unchanged: three states that
        # render identically are worse than three plain badges, because the
        # user would have no way to tell the tray was saying anything.
        pixmap = QPixmap(size, size)
        pixmap.fill(QColor(0, 0, 0, 0))
    if state == STATE_HEALTHY:
        return QIcon(pixmap)

    painter = QPainter(pixmap)
    painter.setRenderHint(QPainter.RenderHint.Antialiasing)
    radius = size * 0.34
    box = QRectF(size - radius * 2, size - radius * 2, radius * 2, radius * 2)

    if state == STATE_BLOCKED:
        painter.setBrush(QColor("#6b7280"))
        painter.setPen(QPen(QColor("#1f2937"), max(1.0, size * 0.03)))
        painter.drawEllipse(box)
        # pause bars
        painter.setPen(QPen(QColor("#ffffff"), max(1.5, size * 0.07),
                            Qt.PenStyle.SolidLine, Qt.PenCapStyle.RoundCap))
        cx, cy = box.center().x(), box.center().y()
        offset = radius * 0.28
        painter.drawLine(int(cx - offset), int(cy - radius * 0.4),
                         int(cx - offset), int(cy + radius * 0.4))
        painter.drawLine(int(cx + offset), int(cy - radius * 0.4),
                         int(cx + offset), int(cy + radius * 0.4))
    else:  # attention
        painter.setBrush(QColor("#dc2626"))
        painter.setPen(QPen(QColor("#7f1d1d"), max(1.0, size * 0.03)))
        painter.drawEllipse(box)
        painter.setPen(QPen(QColor("#ffffff"), max(1.5, size * 0.09),
                            Qt.PenStyle.SolidLine, Qt.PenCapStyle.RoundCap))
        cx, cy = box.center().x(), box.center().y()
        painter.drawLine(int(cx), int(cy - radius * 0.45),
                         int(cx), int(cy + radius * 0.12))
        painter.drawPoint(int(cx), int(cy + radius * 0.45))
    painter.end()
    return QIcon(pixmap)


class Tray(QSystemTrayIcon):
    openRequested = pyqtSignal()
    applyRequested = pyqtSignal()
    dryRunRequested = pyqtSignal()
    revertRequested = pyqtSignal()
    quitRequested = pyqtSignal()

    def __init__(self, base_icon, parent=None):
        super().__init__(parent)
        self._base_icon = base_icon
        self._icons = {state: _badge(base_icon, state)
                       for state in (STATE_HEALTHY, STATE_ATTENTION, STATE_BLOCKED)}

        menu = QMenu()
        self._open = menu.addAction("Open steamtrain")
        self._open.triggered.connect(self.openRequested)
        menu.addSeparator()
        self._apply = menu.addAction("Apply now")
        self._apply.triggered.connect(self.applyRequested)
        self._dry_run = menu.addAction("Dry run")
        self._dry_run.triggered.connect(self.dryRunRequested)
        self._revert = menu.addAction("Revert")
        self._revert.triggered.connect(self.revertRequested)
        menu.addSeparator()
        # Quitting the tray must not disable the scheduled run: the timer is a
        # systemd unit and has nothing to do with whether this process is up.
        self._quit = menu.addAction("Quit (scheduled runs continue)")
        self._quit.triggered.connect(self.quitRequested)
        self.setContextMenu(menu)
        self._menu = menu

        self.activated.connect(self._on_activated)
        self.set_state(STATE_HEALTHY)

    def _on_activated(self, reason):
        if reason in (QSystemTrayIcon.ActivationReason.Trigger,
                      QSystemTrayIcon.ActivationReason.DoubleClick):
            self.openRequested.emit()

    def set_state(self, state, detail=""):
        self.setIcon(self._icons.get(state, self._icons[STATE_HEALTHY]))
        tooltip = TOOLTIPS.get(state, TOOLTIPS[STATE_HEALTHY])
        self.setToolTip(f"{tooltip}\n{detail}" if detail else tooltip)

    def set_actions_enabled(self, enabled, reason=""):
        """Disable write actions with a label saying why, never silently."""
        for action in (self._apply, self._revert):
            action.setEnabled(enabled)
        if not enabled and reason:
            self._apply.setText(f"Apply now — {reason}")
            self._revert.setText(f"Revert — {reason}")
        else:
            self._apply.setText("Apply now")
            self._revert.setText("Revert")

    def notify_changes(self, count):
        """Say something only when something actually changed."""
        if count <= 0:
            return
        self.showMessage(
            "steamtrain",
            f"{count} {'game' if count == 1 else 'games'} updated. "
            f"Restart Steam to see the new launch options.",
            self._base_icon)
