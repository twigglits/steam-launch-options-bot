#!/bin/sh
# Remove steamtrain: units, launcher, package. Steam configs are left as-is;
# run `steamtrain revert` BEFORE uninstalling if you want options restored.
set -eu

systemctl --user disable --now steamtrain.timer 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/steamtrain.service" \
      "$HOME/.config/systemd/user/steamtrain.timer"
systemctl --user daemon-reload 2>/dev/null || true

rm -f "$HOME/.local/bin/steamtrain"
# Kept even though this installer no longer creates it: a user upgrading from a
# release that did still has the directory, and leaving it behind recreates
# exactly the shadowing problem `steamtrain doctor` exists to fix.
rm -rf "$HOME/.local/lib/steamtrain"

echo "Uninstalled. State/backups kept in ~/.local/state/steamtrain"
echo "and config in ~/.config/steamtrain (delete manually if unwanted)."
