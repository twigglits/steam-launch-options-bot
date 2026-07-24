"""Table model over the Core's change records.

Rows are keyed (user, appid) because that is how the Core plans: it iterates
every Steam account on the machine, and a table that collapsed them would
understate what Apply actually writes. The account column is hidden when there
is only one account, which is the common case.

Display metadata comes from `game` records, joined by appid. Change records
carry none: after a revert the Core plans against its state file, which can
hold appids that are no longer installed, and those have no game record at all.
"""

from PyQt6.QtCore import QAbstractTableModel, QModelIndex, Qt

from . import client

COL_ACCOUNT = 0
COL_APPID = 1
COL_NAME = 2
COL_RUNTIME = 3
COL_CURRENT = 4
COL_PROPOSED = 5
COL_STATUS = 6

HEADERS = ["Steam account", "App ID", "Game", "Runtime",
           "Current launch options", "Proposed", "Status"]

# Every status is spelled out in words. Nothing here may be communicated by
# colour alone.
STATUS_TEXT = {
    "set": "Will change",
    "skip-unchanged": "Already applied",
    "skip-user-set": "Kept — you set this",
    "excluded": "Excluded",
}

STATUS_TOOLTIP = {
    "set": "steamtrain will write the proposed options for this game.",
    "skip-unchanged": "The current value already matches what steamtrain proposes.",
    "skip-user-set": "You set this value by hand. steamtrain never overwrites it.",
    "excluded": "This appid is in the `exclude` list, so steamtrain never touches it.",
}


class Row:
    __slots__ = ("user", "appid", "name", "runtime", "current", "proposed", "action")

    def __init__(self, user, appid, name, runtime, current, proposed, action):
        self.user = user
        self.appid = appid
        self.name = name
        self.runtime = runtime
        self.current = current
        self.proposed = proposed
        self.action = action

    @property
    def status_text(self):
        # An unrecognised action is shown verbatim rather than blanked: a newer
        # Core may know actions this interface does not.
        return STATUS_TEXT.get(self.action, self.action)

    @property
    def status_tooltip(self):
        return STATUS_TOOLTIP.get(
            self.action,
            f"Reported by steamtrain as {self.action!r}, which this version of "
            f"the interface does not recognise.")


def rows_from_run(run):
    """Build display rows by joining change records to game records."""
    games = run.games_by_appid()
    rows = []
    for change in run.of_kind(client.KIND_CHANGE):
        appid = change.get("appid", "")
        game = games.get(appid, {})
        rows.append(Row(
            user=change.get("user", ""),
            appid=appid,
            # Falls back to the appid, which is the expected case after a
            # revert against a game that is no longer installed.
            name=game.get("name") or appid,
            runtime=game.get("runtime", ""),
            current=change.get("current", ""),
            proposed=change.get("proposed", ""),
            action=change.get("action", ""),
        ))
    rows.sort(key=lambda r: (r.user, r.name.casefold(), r.appid))
    return rows


class GameTableModel(QAbstractTableModel):
    """Read-only. Editing rules and overrides stays in config.json."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self._rows = []

    def set_rows(self, rows):
        self.beginResetModel()
        self._rows = list(rows)
        self.endResetModel()

    @property
    def accounts(self):
        return {row.user for row in self._rows}

    @property
    def multi_account(self):
        return len(self.accounts) > 1

    def counts_by_action(self):
        counts = {}
        for row in self._rows:
            counts[row.action] = counts.get(row.action, 0) + 1
        return counts

    def rowCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self._rows)

    def columnCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(HEADERS)

    def headerData(self, section, orientation, role=Qt.ItemDataRole.DisplayRole):
        if role != Qt.ItemDataRole.DisplayRole:
            return None
        if orientation == Qt.Orientation.Horizontal:
            return HEADERS[section]
        return section + 1

    def flags(self, index):
        if not index.isValid():
            return Qt.ItemFlag.NoItemFlags
        return Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable

    def data(self, index, role=Qt.ItemDataRole.DisplayRole):
        if not index.isValid():
            return None
        row = self._rows[index.row()]
        column = index.column()

        if role == Qt.ItemDataRole.DisplayRole:
            if column == COL_ACCOUNT:
                return row.user
            if column == COL_APPID:
                return row.appid
            if column == COL_NAME:
                return row.name
            if column == COL_RUNTIME:
                return row.runtime
            if column == COL_CURRENT:
                return row.current or "(empty)"
            if column == COL_PROPOSED:
                return row.proposed or ("—" if row.action == "excluded" else "(empty)")
            if column == COL_STATUS:
                return row.status_text
            return None

        if role == Qt.ItemDataRole.ToolTipRole:
            if column == COL_STATUS:
                return row.status_tooltip
            if column in (COL_CURRENT, COL_PROPOSED):
                return self.data(index, Qt.ItemDataRole.DisplayRole)
            return None

        # Accessible text so a screen reader announces the meaning, not just
        # the visible cell; the status column is the load-bearing one.
        if role == Qt.ItemDataRole.AccessibleTextRole:
            if column == COL_STATUS:
                return f"{row.name}: {row.status_text}"
            return self.data(index, Qt.ItemDataRole.DisplayRole)

        return None
