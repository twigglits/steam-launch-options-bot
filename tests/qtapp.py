"""One QApplication for the whole test process.

Qt allows exactly one QApplication per process, and destroying it takes every
widget with it. Test modules therefore share this one and never tear it down;
a module that dropped its reference would delete widgets another module is
still using, which surfaces as "wrapped C/C++ object has been deleted".
"""

import os
import tempfile

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

try:
    from PyQt6.QtCore import QSettings
    from PyQt6.QtWidgets import QApplication
    HAVE_QT = True
except ImportError:  # the GUI package is optional
    HAVE_QT = False

_app = None
_settings_dir = None


def ensure_app():
    """The shared QApplication, or None when PyQt6 is not installed."""
    global _app, _settings_dir
    if not HAVE_QT:
        return None
    if _app is None:
        # The app records the one-shot legacy-timer migration in QSettings.
        # Pointed at a throwaway directory first, so running the tests cannot
        # touch a developer's own settings file.
        _settings_dir = tempfile.mkdtemp(prefix="steamtrain-tests-")
        QSettings.setDefaultFormat(QSettings.Format.IniFormat)
        QSettings.setPath(QSettings.Format.IniFormat,
                          QSettings.Scope.UserScope, _settings_dir)
        _app = QApplication.instance() or QApplication(["steamtrain-tests"])
        _app.setApplicationName("steamtrain")
        _app.setOrganizationName("steamtrain")
    return _app
