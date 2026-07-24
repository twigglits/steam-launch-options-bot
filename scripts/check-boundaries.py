#!/usr/bin/env python3
"""Enforce the two package boundaries that the whole design rests on.

1. The Core imports nothing outside the standard library. This is what makes
   steamtrain installable and auditable everywhere, and what lets a CLI-only
   or headless user avoid pulling in Qt.

2. The GUI never imports the Core. It reaches it by executing the CLI, so that
   "the window and the timer do the same thing" is true by construction rather
   than by discipline. Checked against import syntax rather than the bare word,
   because the GUI's subprocess adapter necessarily contains the string
   "steamtrain" as an argument.

Run from the repository root:  python3 scripts/check-boundaries.py
"""

import ast
import sys
from pathlib import Path

CORE = Path("steamtrain")
GUI = Path("steamtrain_gui")

# Standard-library modules the Core is allowed to reach for. Deliberately an
# allowlist and not sys.stdlib_module_names: adding to it should be a visible
# decision in a diff, and the check must give the same answer on 3.7 as on 3.13.
ALLOWED = {
    "__future__", "argparse", "ast", "collections", "contextlib", "dataclasses", "datetime",
    "difflib", "errno", "fnmatch", "functools", "glob", "hashlib", "io",
    "itertools", "json", "logging", "os", "pathlib", "platform", "re",
    "shlex", "shutil", "signal", "stat", "subprocess", "sys", "tempfile",
    "textwrap", "time", "typing", "unittest", "urllib", "uuid",
}


def top_level_imports(path):
    """(module, lineno) for every absolute import in one file."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                yield alias.name.split(".")[0], node.lineno
        elif isinstance(node, ast.ImportFrom):
            if node.level:  # relative import, stays inside the package
                continue
            if node.module:
                yield node.module.split(".")[0], node.lineno


def check_core():
    problems = []
    for path in sorted(CORE.glob("*.py")):
        for module, lineno in top_level_imports(path):
            if module not in ALLOWED:
                problems.append(
                    f"{path}:{lineno}: Core imports {module!r}, which is not in the "
                    f"standard-library allowlist. The Core must stay dependency-free; "
                    f"if this really is stdlib, add it to ALLOWED in this script.")
    return problems


def check_gui():
    if not GUI.is_dir():
        return []  # the GUI package does not exist yet
    problems = []
    for path in sorted(GUI.rglob("*.py")):
        for module, lineno in top_level_imports(path):
            if module == "steamtrain":
                problems.append(
                    f"{path}:{lineno}: the GUI imports the Core. It must reach the "
                    f"Core by executing /usr/bin/steamtrain instead, so the window "
                    f"and the systemd timer cannot drift apart.")
    return problems


def main():
    problems = check_core() + check_gui()
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(f"\n{len(problems)} boundary violation(s).", file=sys.stderr)
        return 1
    print("Boundaries intact: Core is stdlib-only, GUI does not import the Core.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
