#!/bin/sh
# Install steamtrain for the current user: one binary, nothing scheduled.
set -eu

REPO_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
BIN_DIR="$HOME/.local/bin"

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

mkdir -p "$BIN_DIR"
install -m 0755 "$REPO_DIR/target/release/steamtrain" "$BIN_DIR/steamtrain"

# An earlier release of this script installed a systemd user timer. It is
# removed rather than left behind: scheduled runs now belong to the settings
# window and last only while it is open, and a stale timer would go on writing
# with nothing of steamtrain on screen.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now steamtrain.timer 2>/dev/null || true
fi
rm -f "$HOME/.config/systemd/user/steamtrain.service" \
      "$HOME/.config/systemd/user/steamtrain.timer"

echo "Installed. Nothing runs on a schedule; nothing has been written to Steam."
echo "  steamtrain scan                                          # see proposals"
echo "  steamtrain apply --dry-run                               # plan without writing"
echo "  steamtrain apply                                         # write them"
echo
echo "To have it run every 30 minutes for newly installed games, open the"
echo "settings window (steamtrain-gui) and leave it open. It runs for as long"
echo "as that window is open, and stops when you close it."

# Hardware setup wizard: only with an interactive terminal on both ends.
if [ -t 0 ] && [ -t 1 ]; then
    "$BIN_DIR/steamtrain" setup || true
else
    echo "Run 'steamtrain setup' to configure hardware (pick your GPU vendor if"
    echo "autodetection failed)."
fi
