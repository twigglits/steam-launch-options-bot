#!/bin/sh
# Install steamtrain for the current user: package, launcher, systemd user units.
set -eu

REPO_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
LIB_DIR="$HOME/.local/lib/steamtrain"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"

usage() {
    echo "usage: $0 [--migrate]"
    echo "  --migrate  remove a previous ~/.local install and exit, installing nothing."
    echo "             Use this when switching to a distribution package. Your config"
    echo "             and state are preserved."
}

# --migrate delegates to `steamtrain doctor`, which owns the removal allowlist.
# Duplicating that list in shell is how the two copies drift apart and one of
# them eventually deletes state.json.
if [ $# -gt 0 ]; then
    case "$1" in
        --migrate)
            if ! command -v python3 >/dev/null 2>&1; then
                echo "ERROR: python3 is required for --migrate." >&2
                exit 1
            fi
            PYTHONPATH="$REPO_DIR${PYTHONPATH:+:$PYTHONPATH}" \
                python3 -m steamtrain doctor --fix --force
            exit $?
            ;;
        -h|--help) usage; exit 0 ;;
        *) echo "ERROR: unknown option $1" >&2; usage >&2; exit 1 ;;
    esac
fi

distro_ids() {
    # Emit "ID ID_LIKE" from os-release for matching; empty if unavailable.
    # Parsed rather than sourced: os-release is shell syntax, so sourcing it
    # would execute whatever the distro shipped in there.
    [ -r /etc/os-release ] || return 0
    sed -n -e 's/^ID=//p' -e 's/^ID_LIKE=//p' /etc/os-release 2>/dev/null |
        tr -d "\"'" | tr '\n' ' '
}

python_install_hint() {
    case "$(distro_ids)" in
        *arch*|*manjaro*)                 echo "  sudo pacman -S python" ;;
        *fedora*|*rhel*|*centos*|*rocky*|*alma*)
                                          echo "  sudo dnf install python3" ;;
        *debian*|*ubuntu*|*mint*|*pop*)   echo "  sudo apt install python3" ;;
        *) echo "  install python3 using your distribution's package manager" ;;
    esac
}

# Preflight: refuse before touching anything when python3 is missing or too old.
if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required but was not found." >&2
    echo "Install it, then re-run ./install.sh:" >&2
    python_install_hint >&2
    exit 1
fi
if ! python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 7) else 1)'; then
    echo "ERROR: python3 >= 3.7 is required (found: $(python3 --version 2>&1))." >&2
    echo "Upgrade it, then re-run ./install.sh:" >&2
    python_install_hint >&2
    exit 1
fi

mkdir -p "$LIB_DIR" "$BIN_DIR" "$UNIT_DIR"

rm -rf "$LIB_DIR/steamtrain"
cp -r "$REPO_DIR/steamtrain" "$LIB_DIR/steamtrain"

cat > "$BIN_DIR/steamtrain" <<EOF
#!/bin/sh
export PYTHONPATH="$LIB_DIR\${PYTHONPATH:+:\$PYTHONPATH}"
exec python3 -m steamtrain "\$@"
EOF
chmod +x "$BIN_DIR/steamtrain"

# systemd user session is optional: install the timer when available, else warn.
# Any systemd step may still fail (no lingering session, masked unit); degrade
# instead of aborting a half-finished install under `set -eu`.
systemd_ok=0
if command -v systemctl >/dev/null 2>&1 && systemctl --user daemon-reload 2>/dev/null; then
    if cp "$REPO_DIR/systemd/steamtrain.service" \
          "$REPO_DIR/systemd/steamtrain.timer" "$UNIT_DIR/" \
        && systemctl --user daemon-reload 2>/dev/null \
        && systemctl --user enable --now steamtrain.timer 2>/dev/null; then
        systemd_ok=1
    fi
fi
if [ "$systemd_ok" = 0 ]; then
    echo "WARNING: systemd user timer not installed; automatic scheduling skipped." >&2
    echo "         The CLI still works - run 'steamtrain apply' yourself (or via" >&2
    echo "         your own scheduler) to set launch options." >&2
fi

echo "Installed. Useful commands:"
echo "  steamtrain scan                                          # see proposals"
echo "  steamtrain apply --dry-run                               # plan without writing"
if [ "$systemd_ok" = 1 ]; then
    echo "  systemctl --user list-timers steamtrain.timer"
    echo "  journalctl --user -u steamtrain.service -e"
else
    echo "  steamtrain apply                                        # no timer; run manually"
fi

# Hardware setup wizard: only with an interactive terminal on both ends.
if [ -t 0 ] && [ -t 1 ]; then
    "$BIN_DIR/steamtrain" setup || true
else
    echo "Run 'steamtrain setup' to configure hardware (pick your GPU vendor if"
    echo "autodetection failed)."
fi
