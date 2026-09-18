#!/bin/sh
#
# The installers ask for six archives by name. The release workflow builds six archives. Fail
# the build when those are not the same six.
#
# **This is the seam nothing else watches.** `install.sh` works out `x86_64-unknown-linux-musl`
# from `uname` and asks the release for `sloop-x86_64-unknown-linux-musl.tar.gz`; the workflow
# decides, separately, which targets to build. Neither one imports the other, and neither one
# fails when they disagree -- the failure lands on a stranger, on the one machine nobody had,
# as a 404 from `curl`. Rule 14: put it in the build.
#
# The six are derived rather than listed here, from the two installers themselves:
#
#   install.sh    two kernels x two architectures  ->  4
#   install.ps1   two architectures                ->  2
#
# so adding a platform to an installer and forgetting the workflow is a red build, and the
# reverse is too.
#
#   usage: sh ci/release-targets.sh

set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
sh_installer="$root/install/install.sh"
ps_installer="$root/install/install.ps1"
workflow="$root/.github/workflows/release.yml"

for file in "$sh_installer" "$ps_installer" "$workflow"; do
    if [ ! -f "$file" ]; then
        printf 'release-targets: no %s\n' "$file" >&2
        exit 1
    fi
done

# What `install.sh` maps a kernel to, and what it maps a machine to. Both are written as
# `name="value"` in one `case` each, which is why they can be read out rather than repeated.
kernels=$(sed -n 's/.*[^_a-z]os="\([a-z0-9._-]*\)".*/\1/p' "$sh_installer" | sort -u)
arches=$(sed -n 's/.*[^_a-z]arch="\([a-z0-9._-]*\)".*/\1/p' "$sh_installer" | sort -u)

# And the two triples `install.ps1` returns outright.
windows=$(sed -n "s/.*return '\([a-z0-9_]*-pc-windows-msvc\)'.*/\1/p" "$ps_installer" | sort -u)

wanted=$(
    for kernel in $kernels; do
        for arch in $arches; do
            printf '%s-%s\n' "$arch" "$kernel"
        done
    done
    printf '%s\n' "$windows"
)
wanted=$(printf '%s\n' "$wanted" | grep -v '^$' | sort -u)

# And what the workflow actually builds.
built=$(sed -n 's/^ *- target: \([a-z0-9_-]*\) *$/\1/p' "$workflow" | sort -u)

# Through files rather than `<(...)`: process substitution is bash, and `/bin/sh` on a
# Debian runner is dash, where it is a syntax error rather than a slower way to do the same
# thing.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
printf '%s\n' "$wanted" >"$work/wanted"
printf '%s\n' "$built" >"$work/built"

missing=$(grep -Fxv -f "$work/built" "$work/wanted" || true)
extra=$(grep -Fxv -f "$work/wanted" "$work/built" || true)

if [ -n "$missing" ] || [ -n "$extra" ]; then
    {
        echo
        echo '  The installers and the release workflow do not name the same targets.'
        echo
        if [ -n "$missing" ]; then
            echo '  An installer will ask for these, and no release will hold them:'
            printf '%s\n' "$missing" | sed 's/^/    /'
            echo
        fi
        if [ -n "$extra" ]; then
            echo '  The workflow builds these, and no installer will ever ask for one:'
            printf '%s\n' "$extra" | sed 's/^/    /'
            echo
        fi
        echo '  Change both, or a machine somebody has gets a 404 from curl.'
        echo
    } >&2
    exit 1
fi

count=$(printf '%s\n' "$wanted" | wc -l | tr -d ' ')
printf 'release-targets: %s targets, named the same by the installers and by the workflow:\n' "$count"
printf '%s\n' "$wanted" | sed 's/^/  /'
