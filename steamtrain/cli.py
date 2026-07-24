"""Command-line interface: scan / apply / status / revert / setup."""

import argparse
import dataclasses
import sys
from pathlib import Path

from . import (__version__, apply as apply_mod, advisor, codes, jsonio, rules,
               steam, sysinfo)


def _build_parser():
    parser = argparse.ArgumentParser(
        prog="steamtrain",
        description="Set hardware-appropriate Steam launch options for installed games.",
    )
    parser.add_argument("--version", action="version", version=f"steamtrain {__version__}")
    sub = parser.add_subparsers(dest="command", required=True)
    for name, help_text in (
        ("scan", "show installed games and proposed launch options"),
        ("apply", "write launch options (skips anything a human set)"),
        ("status", "show what this tool manages and last state"),
        ("revert", "restore every option this tool set back to empty"),
        ("advise", "LLM-propose a per-game override for one appid (needs network + claude)"),
    ):
        p = sub.add_parser(name, help=help_text)
        p.add_argument("--steam-root", default=None, help="Steam root (auto-detected)")
        p.add_argument("--config", default=rules.DEFAULT_CONFIG_PATH)
        p.add_argument("--state-dir", default=apply_mod.DEFAULT_STATE_DIR)
        if name in ("scan", "apply", "status", "revert"):
            p.add_argument("--json", action="store_true",
                           help="machine-readable newline-delimited JSON on stdout")
        if name == "apply":
            p.add_argument("--dry-run", action="store_true", help="plan only, write nothing")
        if name == "advise":
            p.add_argument("game", nargs="?",
                           help="game name (any part, case-insensitive) or appid; "
                                "omit to list installed games")
            p.add_argument("--write", action="store_true",
                           help="save the proposal into config overrides (re-run after reviewing)")
    setup = sub.add_parser(
        "setup", help="confirm detected hardware; pick or clear the GPU vendor if wrong")
    setup.add_argument("--config", default=rules.DEFAULT_CONFIG_PATH)
    return parser


def _emitter(args):
    return jsonio.Emitter(enabled=getattr(args, "json", False))


def _context(args, out=None):
    root = args.steam_root or steam.find_steam_root()
    if root is None:
        if out is not None and out.enabled:
            out.result(False, codes.ERROR, guardrail=codes.NO_STEAM_ROOT,
                       message="no Steam installation found")
        else:
            print("ERROR: no Steam installation found", file=sys.stderr)
        return None
    return Path(root)


_VENDORS = ("nvidia", "amd", "intel")
_VENDOR_NAMES = {"nvidia": "NVIDIA GPU", "amd": "AMD GPU", "intel": "Intel GPU"}


def _profile(config):
    """Detected system profile with the config's gpu_vendor override applied.

    An empty override means autodetect (unchanged behavior); an unrecognized
    value is ignored with a warning and autodetection is used. Matching is
    case-insensitive so the on-screen labels (NVIDIA/AMD/Intel) also work.
    """
    profile = sysinfo.detect()
    raw = config.get("gpu_vendor", "")
    vendor = str(raw or "").strip().lower()
    if not vendor:
        return profile
    if vendor in _VENDORS:
        return dataclasses.replace(
            profile, gpu_vendor=vendor,
            gpu_name=profile.gpu_name or _VENDOR_NAMES[vendor],
            gpu_driver=profile.gpu_driver or "set via steamtrain setup")
    print(f"WARNING: ignoring unrecognized gpu_vendor {raw!r}; using autodetection",
          file=sys.stderr)
    return profile


def _proposals(root, args):
    config = rules.load_config(args.config)
    profile = _profile(config)
    games = steam.installed_games(root)
    options = {}
    names = {}
    excluded = []
    for game in games:
        opts = rules.build_options(game, profile, config)
        if opts is not None:
            options[game.appid] = opts
            names[game.appid] = game.name
        else:
            excluded.append(game.appid)
    return profile, games, options, names, excluded


_PROGRESS_EVERY = 50


def _emit_profile(out, profile):
    out.emit(jsonio.KIND_PROFILE, **dataclasses.asdict(profile))


def _emit_games(out, games):
    """Display metadata only; state lives on change records (AD-15)."""
    for game in games:
        out.emit(jsonio.KIND_GAME, appid=game.appid, name=game.name,
                 runtime=game.runtime, library=str(game.library))


def _excluded_records(root, excluded):
    """One record per (user, appid) so exclusions share change identity.

    The planner never sees these - build_options drops excluded games before
    planning - so they are re-introduced here, which is what lets a client
    show that an exclusion is being honoured rather than losing the game.
    """
    return [{"user": user, "appid": appid, "action": codes.EXCLUDED}
            for user, _ in steam.user_localconfigs(root)
            for appid in excluded]


def _emit_changes(out, changes, extra=()):
    """Emit every change record with progress, and return counts by action."""
    records = [{"user": c.user, "appid": c.appid, "current": c.current,
                "proposed": c.proposed, "action": c.action} for c in changes]
    records.extend(extra)
    total = len(records)
    for index, record in enumerate(records, start=1):
        out.emit(jsonio.KIND_CHANGE, **record)
        if index % _PROGRESS_EVERY == 0 or index == total:
            out.emit(jsonio.KIND_PROGRESS, done=index, total=total)
    counts = {action: 0 for action in codes.ACTIONS}
    for record in records:
        counts[record["action"]] += 1
    return counts


def _print_changes(changes):
    for c in changes:
        marker = {"set": "SET ", "skip-unchanged": "ok  ", "skip-user-set": "KEEP"}[c.action]
        print(f"  [{marker}] user {c.user}  {c.appid:>8}  {c.name}")
        if c.action == "set":
            print(f"           {c.current or '(empty)'!s}  ->  {c.proposed or '(empty)'}")
        elif c.action == "skip-user-set":
            print(f"           keeping human-set value: {c.current}")


def cmd_scan(args):
    out = _emitter(args)
    root = _context(args, out)
    if root is None:
        return 1
    profile, games, options, names, excluded = _proposals(root, args)
    if out.enabled:
        state = apply_mod.State.load(args.state_dir)
        changes = apply_mod.plan_changes(root, options, state, names)
        _emit_profile(out, profile)
        _emit_games(out, games)
        counts = _emit_changes(out, changes, _excluded_records(root, excluded))
        out.result(True, codes.OK, counts=counts,
                   steam_running=steam.is_steam_running(root))
        return 0
    print(f"System: {profile.distro} | {profile.desktop}/{profile.session} | "
          f"{profile.gpu_name} ({profile.gpu_vendor} {profile.gpu_driver}) | "
          f"gamemode={'yes' if profile.has_gamemode else 'no'} "
          f"mangohud={'yes' if profile.has_mangohud else 'no'}")
    print(f"Steam root: {root}  (running: {'yes' if steam.is_steam_running(root) else 'no'})")
    if not games:
        print("No installed games found on mounted libraries.")
        return 0
    print(f"\n{len(games)} installed game(s) on disk:")
    for g in games:
        print(f"  {g.appid:>8}  {g.runtime:<7}  {g.name}")
        print(f"           library: {g.library}")
        print(f"           proposed: {options.get(g.appid, '(excluded)')}")
    return 0


def cmd_apply(args):
    out = _emitter(args)
    root = _context(args, out)
    if root is None:
        return 1
    profile, games, options, names, excluded = _proposals(root, args)
    state = apply_mod.State.load(args.state_dir)
    changes = apply_mod.plan_changes(root, options, state, names)
    if out.enabled:
        return _apply_json(out, args, root, profile, games, changes, excluded)
    _print_changes(changes)
    planned = [c for c in changes if c.action == "set"]
    if args.dry_run:
        print(f"dry-run: {len(planned)} change(s) would be written, nothing touched")
        return 0
    try:
        written = apply_mod.apply_changes(root, changes, args.state_dir)
    except apply_mod.SteamRunningError as exc:
        print(f"NOTE: {exc}")
        return 0  # expected condition; the timer retries later
    print(f"{len(written)} set, {len(changes) - len(written)} skipped")
    return 0


def cmd_status(args):
    out = _emitter(args)
    root = _context(args, out)
    if root is None:
        return 1
    state = apply_mod.State.load(args.state_dir)
    if out.enabled:
        # config_exists is probed directly and never through load_config, which
        # creates the file as a side effect of reading it (AD-6). Reading status
        # must not be what makes the first-run screen stop appearing.
        out.result(True, codes.OK,
                   config_exists=Path(args.config).is_file(),
                   managed=dict(state.data),
                   steam_running=steam.is_steam_running(root))
        return 0
    if not state.data:
        print("No launch options are currently managed by this tool.")
        return 0
    print(f"{len(state.data)} managed launch option(s):")
    for key, value in sorted(state.data.items()):
        print(f"  {key}: {value}")
    return 0


def cmd_revert(args):
    out = _emitter(args)
    root = _context(args, out)
    if root is None:
        return 1
    state = apply_mod.State.load(args.state_dir)
    changes = apply_mod.plan_revert(root, state)
    if out.enabled:
        # No game records here: revert acts on state, which can hold appids that
        # are no longer installed. Clients render those by appid (AD-15).
        counts = _emit_changes(out, changes)
        return _write_json(out, args, root, changes, counts)
    if not changes:
        print("Nothing to revert.")
        return 0
    _print_changes(changes)
    try:
        written = apply_mod.apply_changes(root, changes, args.state_dir)
    except apply_mod.SteamRunningError as exc:
        print(f"NOTE: {exc}")
        return 0
    print(f"{len(written)} reverted")
    return 0


def _resolve_game(games, query):
    """(game, error) — pick one installed game by appid or name substring.

    Exact appid wins; otherwise a case-insensitive name-substring match. Returns
    (None, message) when nothing matches or the name is ambiguous, so the caller
    never has to know an appid.
    """
    for g in games:
        if g.appid == query:
            return g, None
    q = query.casefold()
    matches = [g for g in games if q in g.name.casefold()]
    if len(matches) == 1:
        return matches[0], None
    if not matches:
        return None, f"no installed game matches {query!r}. Run `steamtrain advise` to list them."
    listing = "\n".join(f"  {g.appid:>8}  {g.name}" for g in matches)
    return None, f"{query!r} matches {len(matches)} games; be more specific:\n{listing}"


def _list_installed(games):
    print(f"{len(games)} installed game(s) — run `steamtrain advise <name>`:")
    for g in sorted(games, key=lambda g: g.name.casefold()):
        print(f"  {g.appid:>8}  {g.runtime:<7}  {g.name}")


def cmd_advise(args):
    root = _context(args)
    if root is None:
        return 1
    games = steam.installed_games(root)
    if not games:
        print("No installed games found on mounted libraries.", file=sys.stderr)
        return 1
    if not args.game:
        _list_installed(games)
        return 0
    game, err = _resolve_game(games, args.game)
    if game is None:
        print(f"ERROR: {err}", file=sys.stderr)
        return 1
    config = rules.load_config(args.config)
    profile = _profile(config)
    try:
        prop = advisor.advise(game, profile, config)
    except advisor.AdvisorError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    print(f"{prop.name}  (appid {prop.appid}, {game.runtime})")
    print(f"  baseline  : {prop.baseline}")
    shown = prop.proposed if prop.proposed is not None else "(LLM: baseline already appropriate)"
    print(f"  proposed  : {shown}")
    print(f"  confidence: {prop.confidence}")
    print(f"  reasoning : {prop.reasoning}")
    if prop.proposed is None:
        return 0
    if not prop.valid:
        print(f"  REJECTED by safety check: {prop.warning}", file=sys.stderr)
        print("  Nothing written. Add it to overrides by hand only if you trust it.")
        return 1
    if not args.write:
        print("\nReview looks right? Re-run with --write to save it to overrides.")
        return 0
    rules.save_override(args.config, prop.appid, prop.proposed)
    print(f"\nSaved overrides[{prop.appid}] = {prop.proposed}")
    print("Nothing changed yet — the next `steamtrain apply` (or timer run) applies it.")
    return 0


_SKIP = object()  # menu "Skip": change nothing; distinct from "" (clear override -> autodetect)

_VENDOR_MENU = (
    ("1", "nvidia", "NVIDIA"),
    ("2", "amd", "AMD"),
    ("3", "intel", "Intel"),
    ("4", "", "Autodetect (clear override)"),
    ("5", _SKIP, "Skip (no change)"),
)


def _prompt_gpu_vendor():
    """Numbered menu -> chosen vendor, "" (clear override), or _SKIP; None on EOF.

    KeyboardInterrupt propagates to the caller (exit 130).
    """
    for num, _value, label in _VENDOR_MENU:
        print(f"  {num}) {label}")
    choices = {num: value for num, value, _ in _VENDOR_MENU}
    while True:
        try:
            raw = input(f"Select your GPU vendor [1-{len(_VENDOR_MENU)}]: ").strip()
        except EOFError:
            print()
            return None
        if raw in choices:
            return choices[raw]
        print(f"Please enter a number 1-{len(_VENDOR_MENU)}.")


def _confirm(prompt):
    """[Y/n] confirm; empty input or EOF counts as yes, anything else re-prompts.

    KeyboardInterrupt propagates.
    """
    while True:
        try:
            raw = input(prompt).strip().lower()
        except EOFError:
            print()
            return True
        if raw in ("", "y", "yes"):
            return True
        if raw in ("n", "no"):
            return False
        print("Please answer y or n.")


def _apply_json(out, args, root, profile, games, changes, excluded):
    _emit_profile(out, profile)
    _emit_games(out, games)
    counts = _emit_changes(out, changes, _excluded_records(root, excluded))
    if args.dry_run:
        out.result(True, codes.OK, counts=counts, dry_run=True,
                   steam_running=steam.is_steam_running(root))
        return 0
    return _write_json(out, args, root, changes, counts)


def _write_json(out, args, root, changes, counts):
    """Execute planned changes and close the stream with the run outcome.

    A guardrail refusal exits 0, not non-zero: Steam being open is the expected
    case, and the timer must not record a failure for it (AD-7).
    """
    try:
        written = apply_mod.apply_changes(root, changes, args.state_dir)
    except apply_mod.SteamRunningError as exc:
        out.result(False, codes.BLOCKED, guardrail=codes.STEAM_RUNNING,
                   message=str(exc), counts=counts, written=0)
        return 0
    out.result(True, codes.OK, counts=counts, written=len(written))
    return 0


def cmd_setup(args):
    profile = sysinfo.detect()
    config = rules.load_config(args.config)
    driver = f" {profile.gpu_driver}" if profile.gpu_driver else ""
    print("Detected hardware profile:")
    print(f"  distro : {profile.distro}")
    print(f"  desktop: {profile.desktop} ({profile.session})")
    print(f"  GPU    : {profile.gpu_name or 'unknown'} [{profile.gpu_vendor}]{driver}")
    print(f"  helpers: gamemode={'yes' if profile.has_gamemode else 'no'} "
          f"mangohud={'yes' if profile.has_mangohud else 'no'}")
    try:
        return _setup_interact(args, profile, config.get("gpu_vendor", ""))
    except KeyboardInterrupt:
        print()
        return 130


def _setup_interact(args, profile, override):
    clear_hint = "answer n, then pick 'Autodetect (clear override)' to remove it"

    if profile.gpu_vendor != "unknown":
        if override in _VENDORS:
            print(f"\nGPU autodetected as {profile.gpu_vendor}; config override "
                  f"gpu_vendor={override!r} is active and wins over autodetection "
                  f"({clear_hint}).")
        elif override:
            print(f"\nGPU autodetected as {profile.gpu_vendor}; config value "
                  f"gpu_vendor={override!r} is not recognized and is ignored "
                  f"({clear_hint}).")
        else:
            print(f"\nGPU autodetected as {profile.gpu_vendor}; no override needed.")
        effective = override if override in _VENDORS else profile.gpu_vendor
        if _confirm(f"\nUse {effective} for launch options? [Y/n]: "):
            print(f"Keeping {effective}. Nothing written.")
            return 0
        print("\nChange it — pick the GPU that drives your games:")
    elif override in _VENDORS:
        print(f"\nGPU autodetection failed, but config override gpu_vendor={override!r} "
              "is active — scan/apply/advise already use it.")
        print("Pick a vendor to change it, or Skip to keep it:")
    else:
        if override:
            print(f"\nNOTE: config value gpu_vendor={override!r} is not recognized "
                  "and is ignored.")
        print("\nGPU vendor could not be autodetected. Pick it so scan/apply/advise "
              "set vendor-appropriate options:")

    choice = _prompt_gpu_vendor()
    if choice is None or choice is _SKIP:
        if override in _VENDORS:
            print(f"No change made; override gpu_vendor={override!r} stays in effect.")
        elif override:
            print(f"No change made; unrecognized gpu_vendor={override!r} stays in the "
                  "config (ignored) and autodetection governs.")
        else:
            print("No change made; GPU autodetection stays in effect.")
        return 0
    if choice == "" and not override:
        print("No override set; GPU autodetection is already in effect. Nothing written.")
        return 0
    try:
        rules.save_gpu_vendor(args.config, choice)
    except OSError as exc:
        print(f"ERROR: could not write {args.config}: {exc}", file=sys.stderr)
        return 1
    if choice == "":
        print(f"\nCleared gpu_vendor in {args.config}; GPU autodetection is back in effect.")
    else:
        print(f"\nSaved gpu_vendor={choice!r} to {args.config}.")
    print("Nothing is written to Steam yet — the next `steamtrain apply` (or timer "
          "run) uses it; restart Steam afterwards to see the options in the UI.")
    return 0


COMMANDS = {
    "scan": cmd_scan, "apply": cmd_apply, "status": cmd_status,
    "revert": cmd_revert, "advise": cmd_advise, "setup": cmd_setup,
}


def main(argv=None):
    args = _build_parser().parse_args(argv)
    try:
        return COMMANDS[args.command](args)
    except rules.ConfigError as exc:
        out = _emitter(args)
        if out.enabled:
            out.result(False, codes.ERROR, guardrail=codes.CONFIG_INVALID,
                       message=str(exc))
        else:
            print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
