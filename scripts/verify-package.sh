#!/bin/sh
# Verify an installed steamtrain package inside a throwaway container.
#
# Runs as root in the container. Takes the family-specific install and remove
# commands as arguments so the same checks run identically on every family:
#
#   verify-package.sh "<install command>" "<remove command>"
#
# Checks the properties a package can silently get wrong, in particular that
# installing changes nothing a user owns: a system package must not schedule
# writes to every account's Steam configuration.
set -eu

install_cmd=$1
remove_cmd=$2

echo "--- install"
sh -c "$install_cmd"

echo "--- FR-5: layout"
test -x /usr/bin/steamtrain || { echo "FAIL: /usr/bin/steamtrain missing"; exit 1; }
test -f /usr/lib/systemd/user/steamtrain.timer || { echo "FAIL: user timer missing"; exit 1; }
grep -q '^ExecStart=/usr/bin/steamtrain apply$' /usr/lib/systemd/user/steamtrain.service \
  || { echo "FAIL: unit does not target /usr/bin"; exit 1; }

echo "--- FR-6 / AD-9: installing mutates nothing a user owns"
test ! -e /etc/systemd/user/timers.target.wants/steamtrain.timer \
  || { echo "FAIL: timer was globally enabled"; exit 1; }
useradd -m tester
if find /home/tester -name '*steamtrain*' -print -quit | grep -q .; then
    echo "FAIL: install wrote into a fresh HOME"; exit 1
fi

echo "--- runs as an unprivileged user"
# Privileges are dropped with python3 rather than su: minimal Fedora images
# ship util-linux-core, which has no su, and python3 is guaranteed present
# because it is this package's own dependency.
python3 - <<'PY'
import json, os, pwd, subprocess, sys

user = pwd.getpwnam("tester")
os.setgid(user.pw_gid)
os.setuid(user.pw_uid)
os.environ["HOME"] = user.pw_dir

subprocess.run(["steamtrain", "--version"], check=True)

done = subprocess.run(["steamtrain", "status", "--json"],
                      capture_output=True, text=True)
lines = [line for line in done.stdout.splitlines() if line.strip()]
if not lines:
    sys.exit(f"FAIL: --json produced no records (stderr: {done.stderr})")
try:
    final = json.loads(lines[-1])
except ValueError as exc:
    sys.exit(f"FAIL: last line is not JSON: {exc}\n{lines[-1]}")
if final.get("kind") != "result":
    sys.exit(f"FAIL: stream did not end with a result record: {final}")
print("  --json stream ends with a result record")
PY

echo "--- NFR-8: removal leaves no orphans"
sh -c "$remove_cmd"
test ! -e /usr/bin/steamtrain || { echo "FAIL: binary survived removal"; exit 1; }
test ! -d /usr/lib/steamtrain || { echo "FAIL: lib dir survived removal"; ls -R /usr/lib/steamtrain; exit 1; }

echo "OK"
