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

echo "--- the shipped binary needs no interpreter"
# The Core has no runtime dependency, which is only true if it really is a
# self-contained executable. A wrapper script would pass every other check here
# and then fail on a machine without whatever it invokes.
head -c 4 /usr/bin/steamtrain | grep -q "ELF" \
  || { echo "FAIL: /usr/bin/steamtrain is not an ELF binary"; head -c 64 /usr/bin/steamtrain; exit 1; }

echo "--- runs as an unprivileged user"
# chroot --userspec rather than su, runuser, setpriv or python3. A minimal
# Fedora image has none of those four: util-linux-core ships neither setpriv
# nor su, and python3 is no longer this package's dependency so it may not be
# installed at all. chroot is coreutils, which every image has, and naming the
# user rather than an id avoids the uid differing between distributions.
as_tester() {
    chroot --userspec=tester:tester / \
        env HOME=/home/tester PATH=/usr/bin:/bin "$@"
}

as_tester steamtrain --version

# The exit status is deliberately not asserted here. There is no Steam
# installation in a throwaway container, so this run reports the no-steam-root
# guardrail and exits 1 - which is the correct answer, and exactly the shape a
# client has to cope with. What must hold is that the stream is well formed and
# terminated, whatever the outcome.
as_tester steamtrain status --json > /tmp/status.ndjson || true
test -s /tmp/status.ndjson || { echo "FAIL: --json produced no records"; exit 1; }
last=$(grep -v '^[[:space:]]*$' /tmp/status.ndjson | tail -n 1)
case "$last" in
    *'"kind":"result"'*|*'"kind": "result"'*) ;;
    *) echo "FAIL: stream did not end with a result record: $last"; exit 1 ;;
esac
case "$last" in
    *'"v":1'*|*'"v": 1'*) ;;
    *) echo "FAIL: result record carries no wire version: $last"; exit 1 ;;
esac
echo "  --json stream ends with a versioned result record"

echo "--- installing still wrote nothing into the user's HOME"
# The commands above are the first thing to run as this user, and reading
# status must not create a config file either.
if find /home/tester -name '*steamtrain*' -print -quit | grep -q .; then
    echo "FAIL: a read-only command wrote into HOME"; find /home/tester -name '*steamtrain*'; exit 1
fi

echo "--- NFR-8: removal leaves no orphans"
sh -c "$remove_cmd"
test ! -e /usr/bin/steamtrain || { echo "FAIL: binary survived removal"; exit 1; }
test ! -d /usr/lib/steamtrain || { echo "FAIL: lib dir survived removal"; ls -R /usr/lib/steamtrain; exit 1; }

echo "OK"
