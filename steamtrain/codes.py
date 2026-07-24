"""Closed vocabulary of machine-readable codes for the --json output mode.

Clients switch on these constants. Every code may be accompanied by a
human-readable `message`, but the message is for display only: it is not
stable and must never be parsed. Adding a code here is additive; renaming
one is a wire-format break and needs a `jsonio.VERSION` bump.
"""

# Run-level guardrails: why a whole invocation declined to act. Each names the
# condition ("steam-running"), never the consequence ("cannot-write").
STEAM_RUNNING = "steam-running"
NO_STEAM_ROOT = "no-steam-root"
CONFIG_INVALID = "config-invalid"
NO_SYSTEMD_SESSION = "no-systemd-session"
LEGACY_INSTALL_SHADOWING = "legacy-install-shadowing"

GUARDRAILS = (
    STEAM_RUNNING,
    NO_STEAM_ROOT,
    CONFIG_INVALID,
    NO_SYSTEMD_SESSION,
    LEGACY_INSTALL_SHADOWING,
)

# Change-level actions: what happened to one (user, appid) pair. These mirror
# apply.Change.action exactly, plus 'excluded', which the planner never
# produces because excluded games are dropped before planning - the JSON layer
# re-introduces them so a client can show the exclusion is being honoured.
SET = "set"
SKIP_USER_SET = "skip-user-set"
SKIP_UNCHANGED = "skip-unchanged"
EXCLUDED = "excluded"

ACTIONS = (SET, SKIP_USER_SET, SKIP_UNCHANGED, EXCLUDED)

# Run outcomes, carried on the final `result` record.
OK = "ok"          # ran to completion; see counts for what it did
BLOCKED = "blocked"  # a guardrail declined the write; exit status is still 0
ERROR = "error"      # something actually went wrong

OUTCOMES = (OK, BLOCKED, ERROR)
