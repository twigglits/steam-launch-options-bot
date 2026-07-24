"""One QApplication for the whole test process.

Qt allows exactly one QApplication per process, and destroying it takes every
widget with it. Test modules therefore share this one and never tear it down;
a module that dropped its reference would delete widgets another module is
still using, which surfaces as "wrapped C/C++ object has been deleted".
"""

import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

try:
    from PyQt6.QtWidgets import QApplication
    HAVE_QT = True
except ImportError:  # the GUI package is optional
    HAVE_QT = False

_app = None


def ensure_app():
    """The shared QApplication, or None when PyQt6 is not installed."""
    global _app
    if not HAVE_QT:
        return None
    if _app is None:
        _app = QApplication.instance() or QApplication(["steamtrain-tests"])
    return _app
