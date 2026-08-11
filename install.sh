#!/bin/sh
# Install steamtrain for the current user: binary, systemd user units.
set -eu

REPO_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"

usage() {
    echo "usage: $0 [--migrate]"
    echo "  --migrate  remove a previous ~/.local install and exit, installing nothing."
    echo "             Use this when switching to a distribution package. Your config"
    echo "             and state are preserved."
}

distro_ids() {
    # Emit "ID ID_LIKE" from os-release for matching; empty if unavailable.
    # Parsed rather than sourced: os-release is shell syntax, so sourcing it
    # would execute whatever the distro shipped in there.
    [ -r /etc/os-release ] || return 0
    sed -n -e 's/^ID=//p' -e 's/^ID_LIKE=//p' /etc/os-release 2>/dev/null |
        tr -d "\"'" | tr '\n' ' '
}

cargo_install_hint() {
    case "$(distro_ids)" in
        *arch*|*manjaro*)                 echo "  sudo pacman -S rust" ;;
        *fedora*|*rhel*|*centos*|*rocky*|*alma*)
                                          echo "  sudo dnf install cargo" ;;
        *debian*|*ubuntu*|*mint*|*pop*)   echo "  sudo apt install cargo" ;;
        *) echo "  install Rust from https://rustup.rs" ;;
    esac
    echo "Or install a distribution package instead, which needs no toolchain:"
    echo "  https://github.com/twigglits/steamtrain/releases/latest"
}

require_cargo() {
    command -v cargo >/dev/null 2>&1 && return 0
    echo "ERROR: cargo (Rust) is required to build steamtrain from source." >&2
    echo "Install it, then re-run $1:" >&2
    cargo_install_hint >&2
    return 1
}

build() {
    ( cd "$REPO_DIR" && cargo build --release )
}

# --migrate delegates to `steamtrain doctor`, which owns the removal allowlist.
# Duplicating that list in shell is how the two copies drift apart and one of
# them eventually deletes state.json. An already-installed binary is preferred
# over building one, so switching to a distribution package does not require a
# toolchain the user may not have.
if [ $# -gt 0 ]; then
    case "$1" in
        --migrate)
            if command -v steamtrain >/dev/null 2>&1; then
                exec steamtrain doctor --fix --force
            fi
            require_cargo "$0 --migrate"
            build
            exec "$REPO_DIR/target/release/steamtrain" doctor --fix --force
            ;;
        -h|--help) usage; exit 0 ;;
        *) echo "ERROR: unknown option $1" >&2; usage >&2; exit 1 ;;
    esac
fi

# Preflight: refuse before touching anything when cargo is missing.
require_cargo "./install.sh"

build

mkdir -p "$BIN_DIR" "$UNIT_DIR"
install -m 0755 "$REPO_DIR/target/release/steamtrain" "$BIN_DIR/steamtrain"

# systemd user session is optional: install the timer when available, else warn.
# Any systemd step may still fail (no lingering session, masked unit); degrade
# instead of aborting a half-finished install under `set -eu`.
#
# The unit is written but deliberately NOT enabled. An installer that schedules
# writes to your Steam configuration without being asked is exactly the hidden
# state this tool must not have: after this script, nothing runs until you say
# so, either here or in the settings window.
systemd_ok=0
if command -v systemctl >/dev/null 2>&1 && systemctl --user daemon-reload 2>/dev/null; then
    # The shipped unit targets /usr/bin, which is where a distribution package
    # puts the binary. This install puts it in ~/.local, so the ExecStart is
    # rewritten on the way in rather than maintaining a second copy of the unit.
    if sed 's,^ExecStart=/usr/bin/steamtrain ,ExecStart=%h/.local/bin/steamtrain ,' \
           "$REPO_DIR/systemd/steamtrain.service" > "$UNIT_DIR/steamtrain.service" \
        && cp "$REPO_DIR/systemd/steamtrain.timer" "$UNIT_DIR/" \
        && systemctl --user daemon-reload 2>/dev/null; then
        systemd_ok=1
    fi
fi
if [ "$systemd_ok" = 0 ]; then
    echo "WARNING: systemd user timer not installed; there is no way to schedule" >&2
    echo "         runs. The CLI still works - run 'steamtrain apply' yourself." >&2
fi

echo "Installed. Nothing runs on a schedule; nothing has been written to Steam."
echo "  steamtrain scan                                          # see proposals"
echo "  steamtrain apply --dry-run                               # plan without writing"
echo "  steamtrain apply                                         # write them"
if [ "$systemd_ok" = 1 ]; then
    echo
    echo "To have it run every 30 minutes for newly installed games:"
    echo "  systemctl --user enable --now steamtrain.timer         # opt in"
    echo "  systemctl --user list-timers steamtrain.timer          # confirm it is on"
    echo "  systemctl --user disable --now steamtrain.timer        # opt back out"
fi

# Hardware setup wizard: only with an interactive terminal on both ends.
if [ -t 0 ] && [ -t 1 ]; then
    "$BIN_DIR/steamtrain" setup || true
else
    echo "Run 'steamtrain setup' to configure hardware (pick your GPU vendor if"
    echo "autodetection failed)."
fi
