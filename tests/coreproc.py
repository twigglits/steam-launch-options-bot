"""Locating and fixturing the Core for the Python-side tests.

The Core is a compiled binary in another language now, so the tests that need
it execute it. This is the Python counterpart of `tests/support/mod.rs`: the
one place that knows where the binary is and how to build a Steam root to point
it at, so no test module has to import helpers out of another test module.
"""

import os
import shutil
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

SKIP_REASON = ("no steamtrain binary; run `cargo build` or set STEAMTRAIN_BIN")


def find_core():
    """Path to the steamtrain binary, or None.

    STEAMTRAIN_BIN is what CI sets after `cargo build`; the debug and release
    builds and then PATH are the convenient fallbacks for a developer.
    """
    override = os.environ.get("STEAMTRAIN_BIN")
    if override:
        return override if Path(override).is_file() else None
    for candidate in (REPO_ROOT / "target/debug/steamtrain",
                      REPO_ROOT / "target/release/steamtrain"):
        if candidate.is_file():
            return str(candidate)
    return shutil.which("steamtrain")


CORE = find_core()


def make_steam_root(base):
    """A Steam root with a steamapps directory and an empty libraryfolders.vdf."""
    root = Path(base) / "Steam"
    (root / "steamapps" / "common").mkdir(parents=True)
    (root / "steamapps" / "libraryfolders.vdf").write_text('"libraryfolders"\n{\n}\n')
    return root


def make_manifest(root, appid, name, installdir):
    """An appmanifest plus the install folder it points at."""
    steamapps = Path(root) / "steamapps"
    (steamapps / "common" / installdir).mkdir(parents=True, exist_ok=True)
    (steamapps / f"appmanifest_{appid}.acf").write_text(
        f'"AppState"\n{{\n\t"appid"\t\t"{appid}"\n'
        f'\t"name"\t\t"{name}"\n\t"installdir"\t\t"{installdir}"\n}}\n')


def make_user(root, account):
    """A localconfig.vdf for one Steam account."""
    config = Path(root) / "userdata" / account / "config"
    config.mkdir(parents=True)
    path = config / "localconfig.vdf"
    path.write_text('"UserLocalConfigStore"\n{\n}\n')
    return path


def make_bindir(base):
    """A directory holding only `steamtrain`, for use as a whole PATH.

    A symlink to the binary under test rather than whatever `steamtrain`
    happens to resolve to on the developer's PATH, which may be an older
    install entirely.
    """
    bindir = Path(base) / "bin"
    bindir.mkdir(exist_ok=True)
    link = bindir / "steamtrain"
    if not link.exists():
        os.symlink(CORE, link)
    return str(bindir)
