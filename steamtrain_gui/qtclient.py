"""Thin Qt wrapper around client.py. No logic of its own.

Every Core invocation runs off the Qt main thread and delivers its records
back as signals, so a scan across a large library never freezes the window.
Qt queues signals emitted from a worker thread onto the receiver's thread, so
slots connected here run on the GUI thread and may touch widgets safely.
"""

from PyQt6.QtCore import QObject, QRunnable, QThreadPool, pyqtSignal

from . import client


class CoreSignals(QObject):
    record = pyqtSignal(dict)     # every record, as it arrives
    finished = pyqtSignal(object)  # client.Run
    failed = pyqtSignal(str)       # human-readable, already formatted


class _CoreTask(QRunnable):
    def __init__(self, args, signals):
        super().__init__()
        self._args = args
        self._signals = signals

    def run(self):
        try:
            result = client.stream(self._args, on_record=self._signals.record.emit)
        except client.CoreNotFound as exc:
            self._signals.failed.emit(str(exc))
        except client.ProtocolError as exc:
            self._signals.failed.emit(str(exc))
        except OSError as exc:
            self._signals.failed.emit(f"could not run steamtrain: {exc}")
        else:
            self._signals.finished.emit(result)


class CoreRunner(QObject):
    """Runs one Core command at a time, off the main thread."""

    record = pyqtSignal(dict)
    finished = pyqtSignal(object)
    failed = pyqtSignal(str)
    busyChanged = pyqtSignal(bool)

    def __init__(self, parent=None, pool=None):
        super().__init__(parent)
        self._pool = pool or QThreadPool.globalInstance()
        self._busy = False

    @property
    def busy(self):
        return self._busy

    def start(self, args):
        """Begin a command. Ignored while one is already running.

        Refusing rather than queueing is deliberate: the actions that reach
        here write to Steam's config, and two of them overlapping is never
        what the user meant.
        """
        if self._busy:
            return False
        self._busy = True
        self.busyChanged.emit(True)

        # Parented, so Qt owns its lifetime: an unparented signals object can
        # be collected while the worker thread is still emitting through it,
        # which surfaces as "wrapped C/C++ object has been deleted" or a
        # segfault during interpreter shutdown.
        signals = CoreSignals(self)
        signals.record.connect(self.record)
        signals.finished.connect(self._on_finished)
        signals.failed.connect(self._on_failed)
        self._signals = signals  # keep alive for the task's lifetime
        self._pool.start(_CoreTask(list(args), signals))
        return True

    def _on_finished(self, result):
        self._busy = False
        self.busyChanged.emit(False)
        self.finished.emit(result)

    def _on_failed(self, message):
        self._busy = False
        self.busyChanged.emit(False)
        self.failed.emit(message)

    def wait(self, msecs=5000):
        """Block until any in-flight command finishes.

        Called on shutdown: a worker still parsing the Core's output when the
        widgets are destroyed is a crash on exit, not a tidy quit.
        """
        return self._pool.waitForDone(msecs)
