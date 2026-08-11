#!/usr/bin/env python3
"""Enforce the two package boundaries that the whole design rests on.

1. The Core's dependency surface stays small and argued-for. It used to be
   "imports nothing outside the Python standard library"; now that the Core is
   Rust it is "depends on nothing outside a committed allowlist of crates". The
   rule is the same one: steamtrain has to stay installable and auditable
   everywhere, and a CLI-only or headless user must not end up pulling in Qt.

2. The GUI never imports the Core. It reaches it by executing the CLI, so that
   "the window and the timer do the same thing" is true by construction rather
   than by discipline. Checked against import syntax rather than the bare word,
   because the GUI's subprocess adapter necessarily contains the string
   "steamtrain" as an argument. With the Core in another language this is now
   also impossible by accident - which is a reason to keep checking it, not to
   stop: the GUI could still grow a `steamtrain` Python package dependency.

Run from the repository root:  python3 scripts/check-boundaries.py
"""

import ast
import sys
from pathlib import Path

CARGO_TOML = Path("Cargo.toml")
GUI = Path("steamtrain_gui")

# Crates the Core is allowed to depend on. Deliberately an allowlist: adding
# one should be a visible decision in a diff, exactly as adding a standard
# library module was before the Core was Rust.
#
#   clap        - argument parsing
#   serde       - derive support for serde_json
#   serde_json  - config, state and the --json wire format
#   shlex       - POSIX splitting for the override safety gate
ALLOWED_CRATES = {"clap", "serde", "serde_json", "shlex"}


def declared_dependencies():
    """(crate, lineno) for every entry in Cargo.toml's [dependencies].

    A line scan rather than a TOML parse: tomllib is 3.11+, and this script
    should keep giving the same answer on whatever Python a contributor has.
    The section is a flat table of `name = ...` lines.
    """
    inside = False
    text = CARGO_TOML.read_text(encoding="utf-8")
    for lineno, line in enumerate(text.splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("["):
            inside = stripped == "[dependencies]"
            continue
        if not inside or not stripped or stripped.startswith("#"):
            continue
        name, separator, _ = stripped.partition("=")
        if separator:
            yield name.strip(), lineno


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
    if not CARGO_TOML.is_file():
        return [f"{CARGO_TOML} is missing; the Core cannot be checked."]
    problems = []
    declared = list(declared_dependencies())
    if not declared:
        problems.append(
            f"{CARGO_TOML}: no [dependencies] section was found. If the Core "
            f"genuinely has no dependencies, delete this check; more likely "
            f"the manifest moved and this script is now checking nothing.")
    for crate, lineno in declared:
        if crate not in ALLOWED_CRATES:
            problems.append(
                f"{CARGO_TOML}:{lineno}: Core depends on {crate!r}, which is not "
                f"in the allowlist. The Core's dependency surface is deliberately "
                f"small; if this crate is genuinely needed, add it to "
                f"ALLOWED_CRATES here and say why in the commit.")
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
    print("Boundaries intact: Core dependencies are on the allowlist, "
          "GUI does not import the Core.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
