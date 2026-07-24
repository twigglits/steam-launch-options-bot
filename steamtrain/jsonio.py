"""Newline-delimited JSON output envelope for the --json mode.

One JSON object per line on stdout, never a wrapping array, so a client can
render a long run as it streams instead of waiting for the end. Every record
carries {"v": VERSION, "kind": ...}; the final record of every invocation is
always a `result`, which is what makes a truncated stream detectable.

Nothing but these records may reach stdout while --json is active; warnings
and diagnostics go to stderr, where the systemd journal already collects them.
"""

import json
import sys

VERSION = 1

KIND_PROFILE = "profile"
KIND_GAME = "game"
KIND_CHANGE = "change"
KIND_FINDING = "finding"
KIND_PROGRESS = "progress"
KIND_RESULT = "result"

KINDS = (
    KIND_PROFILE,
    KIND_GAME,
    KIND_CHANGE,
    KIND_FINDING,
    KIND_PROGRESS,
    KIND_RESULT,
)


class Emitter:
    """Writes envelope records, or nothing at all when disabled.

    A disabled emitter is a no-op so callers can emit unconditionally and let
    the text-mode branch do its own printing; this keeps the two output modes
    from growing separate control flow.
    """

    def __init__(self, stream=None, enabled=True):
        self.enabled = enabled
        self._stream = stream if stream is not None else sys.stdout
        self._finished = False

    def emit(self, kind, **fields):
        if not self.enabled:
            return
        if kind not in KINDS:
            raise ValueError(f"unknown record kind {kind!r}")
        if kind == KIND_RESULT:
            raise ValueError("emit a result with result(), not emit()")
        if self._finished:
            raise RuntimeError("records cannot follow the result record")
        self._write({"v": VERSION, "kind": kind}, fields)

    def result(self, ok, outcome, **fields):
        """Emit the terminal record. Exactly one per invocation."""
        if not self.enabled:
            return
        if self._finished:
            raise RuntimeError("result already emitted")
        self._finished = True
        self._write({"v": VERSION, "kind": KIND_RESULT, "ok": bool(ok),
                     "outcome": outcome}, fields)

    def _write(self, head, fields):
        record = dict(head)
        record.update(fields)
        self._stream.write(json.dumps(record) + "\n")
        self._stream.flush()  # a client streaming progress must see it now
