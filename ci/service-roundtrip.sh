#!/bin/sh
#
# Register sloop with this machine's service manager, drive it, and take it off again.
#
# **Rule 14, and this one could not be checked any other way.** A systemd unit that systemd
# refuses, a launchd plist launchd will not load, a `User=` naming an account that does not
# exist -- none of those are visible in the text of the file, and the text is all a unit test
# can see. So the real service manager is asked, on a machine that has one.
#
# Everything sloop's own tests already prove about the *contents* is left to them; what is
# asserted here is the half that needs an init system:
#
#   install        the manager accepts the definition and enables it for boot
#   start / stop   it goes to running and back, and says so honestly both times
#   status         stopped and never-installed come back as different answers
#   uninstall      the unit file, the registration and the key file are all gone
#
# **What is not asserted, because a CI runner cannot be restarted:** that it comes back after
# a reboot. What stands in for it is `starts_at_boot` -- systemd `is-enabled`, launchd's
# `RunAtLoad` -- which is the thing a reboot would depend on.
#
#   usage: sudo sh ci/service-roundtrip.sh [path/to/sloop]

set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
sloop="${1:-$root/cli/target/debug/sloop}"

if [ ! -x "$sloop" ]; then
    printf 'service-roundtrip: no binary at %s\n' "$sloop" >&2
    exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
    printf 'service-roundtrip: registering something to start at boot is a machine-wide\n' >&2
    printf '  change on all three platforms. Run this with sudo.\n' >&2
    exit 1
fi

case "$(uname -s)" in
Linux)
    if ! command -v systemctl >/dev/null 2>&1; then
        printf 'service-roundtrip: no systemctl, so there is nothing to register with\n' >&2
        exit 1
    fi
    definition=/etc/systemd/system/sloop.service
    ;;
Darwin) definition=/Library/LaunchDaemons/io.github.dbsloop.sloop.plist ;;
*)
    printf 'service-roundtrip: this script is for Linux and macOS\n' >&2
    exit 1
    ;;
esac

key=/etc/sloop/service.key

checks=0
passed=0
failed=0

record() {
    checks=$((checks + 1))
    if [ "$1" -eq 0 ]; then
        passed=$((passed + 1))
        printf '  ok    %s\n' "$2"
    else
        failed=1
        printf '  FAIL  %s\n' "$2" >&2
    fi
}

check() {
    label="$1"
    shift
    if "$@" >/dev/null 2>&1; then record 0 "$label"; else record 1 "$label"; fi
}

check_not() {
    label="$1"
    shift
    if "$@" >/dev/null 2>&1; then record 1 "$label"; else record 0 "$label"; fi
}

check_same() {
    if [ "$2" = "$3" ]; then record 0 "$1"; else record 1 "$1 -- expected [$3], got [$2]"; fi
}

# What `sloop service status` says on the `State` line, as one word-ish string.
state() {
    "$sloop" service status 2>/dev/null | sed -n 's/^ *State *//p' | head -n 1
}

# **Left clean whatever happens.** A CI runner is reused within a job, and a half-installed
# service left behind by a failed assertion would make every later step lie.
#
# shellcheck disable=SC2329  # invoked by the trap below, which shellcheck cannot see
cleanup() {
    "$sloop" service uninstall >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

printf 'service-roundtrip: %s, against the real service manager\n\n' "$(uname -s)"

# --------------------------------------------------------------------------- before

check_same "nothing installed, and it says so" "$(state)" "not installed"
check "status exits 0 even with nothing installed" "$sloop" service status
check_not "start refuses when there is nothing to start" "$sloop" service start

# -------------------------------------------------------------------------- install

if ! "$sloop" service install >/tmp/install.log 2>&1; then
    sed 's/^/  | /' /tmp/install.log >&2
    printf 'service-roundtrip: install failed\n' >&2
    exit 1
fi
sed 's/^/  | /' /tmp/install.log
printf '\n'

check "the definition is on disk" test -f "$definition"
check_same "it is running" "$(state)" "running"
check "the machine agrees it will start at boot" \
    sh -c "\"$sloop\" service status | grep -Fq 'starts by itself'"

# The key file is the whole of the protection for the passphrase, so its mode is asserted
# rather than assumed. `find -perm` rather than reading `ls`, which prints a different
# string on BSD and on GNU and is the one thing both platforms are here to disagree about.
check "the key file is readable by its owner and nobody else"     sh -c "test -n \"\$(find '$key' -maxdepth 0 -perm 600 2>/dev/null)\""
check_not "and nobody else can read it"     sh -c "test -n \"\$(find '$key' -maxdepth 0 -perm /077 2>/dev/null)\""

# Rule 3: a unit file is world-readable on both of these platforms.
check_not "the definition does not contain the passphrase" \
    sh -c "grep -Fq \"\$(cat '$key')\" '$definition'"

# ------------------------------------------------------------------------ stop, start

"$sloop" service stop >/dev/null
check_same "stopped, and it is not confused with never installed" "$(state)" "installed, stopped"
check "it is still registered for the next boot while stopped" \
    sh -c "\"$sloop\" service status | grep -Fq 'starts by itself'"

"$sloop" service start >/dev/null
check_same "started again" "$(state)" "running"

# Installing over an install is what an upgrade does.
check "installing twice is not an error" "$sloop" service install

# ------------------------------------------------------------------------- uninstall

"$sloop" service uninstall >/tmp/uninstall.log 2>&1
sed 's/^/  | /' /tmp/uninstall.log
printf '\n'

check_not "the definition is gone" test -e "$definition"
check_not "the key file is gone" test -e "$key"
check_same "and status says so" "$(state)" "not installed"
check "uninstalling twice is not an error" "$sloop" service uninstall

if [ "$(uname -s)" = "Linux" ]; then
    check_not "systemd has forgotten the unit" \
        sh -c "systemctl is-enabled sloop 2>/dev/null | grep -q enabled"
fi

printf '\nservice-roundtrip: %s of %s checks passed.\n' "$passed" "$checks"
exit "$failed"
