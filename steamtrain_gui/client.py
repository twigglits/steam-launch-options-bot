"""Talk to the steamtrain CLI. The only place in the GUI that spawns it.

The GUI is a client process of the CLI, never a library consumer: it never
imports the Core. That is what makes "the window and the systemd timer do the
same thing" true by construction instead of by discipline, and it keeps the
Core's internals private with no stability contract to honour.

Deliberately free of Qt so the protocol handling can be tested headlessly. The
Qt layer on top of this is a thin threading and signal wrapper, nothing more.
"""

import json
import shutil
import subprocess

PROTOCOL_VERSION = 1

# Mirrors of the Core's vocabulary. Duplicated rather than imported, because
# importing the Core is exactly what this design forbids; the wire format is
# the contract, and `v` guards it. Unknown values must degrade, never crash.
KIND_PROFILE = "profile"
KIND_GAME = "game"
KIND_CHANGE = "change"
KIND_FINDING = "finding"
KIND_PROGRESS = "progress"
KIND_RESULT = "result"

OUTCOME_OK = "ok"
OUTCOME_BLOCKED = "blocked"
OUTCOME_ERROR = "error"


class CoreNotFound(Exception):
    """No steamtrain executable on PATH."""


class ProtocolError(Exception):
    """The Core produced something this client cannot interpret."""


class Run:
    """The records of one completed CLI invocation."""

    def __init__(self, records, returncode, stderr=""):
        self.records = records
        self.returncode = returncode
        self.stderr = stderr

    @property
    def result(self):
        """The terminal record. Absent means the stream was truncated."""
        for record in reversed(self.records):
            if record.get("kind") == KIND_RESULT:
                return record
        return None

    def of_kind(self, kind):
        return [r for r in self.records if r.get("kind") == kind]

    @property
    def ok(self):
        result = self.result
        return bool(result and result.get("ok"))

    @property
    def blocked(self):
        result = self.result
        return bool(result and result.get("outcome") == OUTCOME_BLOCKED)

    @property
    def guardrail(self):
        """Machine code for why this run refused, or None."""
        result = self.result
        return result.get("guardrail") if result else None

    @property
    def message(self):
        """Human text to display verbatim. Never parse this."""
        result = self.result
        return (result or {}).get("message", "")

    def games_by_appid(self):
        """Display metadata, keyed for joining to change records.

        Change records deliberately carry no name: after a revert the Core
        plans against state, which can hold appids that are no longer
        installed, and those have no game record at all.
        """
        return {g["appid"]: g for g in self.of_kind(KIND_GAME) if "appid" in g}


def find_core(executable="steamtrain"):
    """Absolute path to the Core CLI, by PATH lookup.

    Never a hardcoded /usr/bin path, so a developer checkout works unchanged.
    """
    found = shutil.which(executable)
    if found is None:
        raise CoreNotFound(
            f"{executable!r} is not on PATH. Install the steamtrain package.")
    return found


def parse_stream(text):
    """NDJSON text to records, tolerating anything a future Core adds.

    Unknown record kinds and unknown codes are passed through rather than
    rejected: the client must degrade on a newer Core, not crash.
    """
    records = []
    for number, line in enumerate(text.splitlines(), start=1):
        line = line.strip()
        if not line:
            continue
        try:
            record = json.loads(line)
        except ValueError as exc:
            raise ProtocolError(f"line {number} is not JSON: {exc}") from exc
        if not isinstance(record, dict):
            raise ProtocolError(f"line {number} is not a JSON object")
        version = record.get("v")
        if version is not None and version > PROTOCOL_VERSION:
            raise ProtocolError(
                f"the installed steamtrain speaks wire format v{version}, but "
                f"this interface understands v{PROTOCOL_VERSION}. The two "
                f"packages are mismatched; install matching versions.")
        records.append(record)
    return records


def run(args, executable="steamtrain", timeout=120, runner=None):
    """Run one CLI command in --json mode and collect its records.

    A non-zero exit is not raised on: a blocked run exits 0 by design, and
    `doctor` exits 2 for findings it did not fix. Callers decide using the
    result record, which is why the exit status alone is never enough.
    """
    argv = [find_core(executable), *args, "--json"]
    runner = runner or _default_runner
    completed = runner(argv, timeout)
    records = parse_stream(completed.stdout)
    if not records:
        raise ProtocolError(
            "the Core produced no records at all"
            + (f" (stderr: {completed.stderr.strip()})" if completed.stderr else ""))
    run_result = Run(records, completed.returncode, completed.stderr)
    if run_result.result is None:
        raise ProtocolError(
            "the Core's output ended without a result record, so the run was "
            "cut short and its outcome is unknown")
    return run_result


def _default_runner(argv, timeout):
    return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)


def core_version(executable="steamtrain", runner=None):
    """Version string of the installed Core, for the parity check."""
    runner = runner or _default_runner
    completed = runner([find_core(executable), "--version"], 30)
    # argparse prints "steamtrain X.Y.Z"
    parts = completed.stdout.strip().split()
    if len(parts) < 2:
        raise ProtocolError(f"unexpected --version output: {completed.stdout!r}")
    return parts[-1]
