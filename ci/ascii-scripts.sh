#!/bin/sh
#
# Fail the build if an install script has a byte above 0x7E in it.
#
# **This is not tidiness, it is a script that stops working.** Windows PowerShell 5.1 reads a
# `.ps1` with no byte-order mark as the system's ANSI codepage. A UTF-8 em dash then arrives
# as three CP1252 characters, and one of them is U+201D -- which PowerShell accepts as a
# string delimiter. A comment containing a dash ends a string early, and the script dies on a
# brace mismatch fifty lines from the character that caused it. It cost a debugging session
# once; it is a one-line check now.
#
# A BOM would fix the file on disk and break it through `irm | iex`, so the rule is the other
# one: no character above 0x7E, anywhere in these files.
#
# **The shell installers are held to it as well**, for a smaller reason: they print their
# output on whatever locale the machine happens to have, and a dash that arrives as mojibake
# in the one message telling somebody to restart their shell is a message that reads as a
# fault.
#
# Everything under `install/` is in scope because all of it is served over HTTPS and piped
# into a shell, and `ci/*.ps1` because those run under the same PowerShell 5.1. The rest of
# `ci/` is not: it runs on a Linux runner in a UTF-8 locale and never reaches a user.
#
#   usage: sh ci/ascii-scripts.sh

set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)

found=''
checked=0

for file in "$root"/install/* "$root"/ci/*.ps1; do
    [ -f "$file" ] || continue
    checked=$((checked + 1))

    # `LC_ALL=C` makes the bracket expression bytes rather than characters, which is the
    # level this has to be checked at. Tab is allowed; nothing else outside printable ASCII.
    #
    # **The carriage returns come off first**, because `.gitattributes` checks a `.ps1` out
    # with CRLF on purpose -- Windows is what runs it -- and a check that called its own
    # line endings a violation would fail on every line of every file it is meant to guard.
    if lines=$(tr -d '\r' <"$file" | LC_ALL=C grep -n '[^ -~	]'); then
        found="$found$file
$lines
"
    fi
done

if [ -n "$found" ]; then
    {
        echo
        echo '  An install script is not ASCII.'
        echo
        echo '  Windows PowerShell 5.1 reads a .ps1 without a BOM as the ANSI codepage, and'
        echo '  a UTF-8 dash becomes a character PowerShell treats as a string delimiter.'
        echo '  The script then fails to parse, for a reason invisible in the diff.'
        echo
        printf '%s' "$found" | sed 's/^/    /'
        echo
        echo '  Write -- for an em dash, ... for an ellipsis, and a plain apostrophe.'
        echo
    } >&2
    exit 1
fi

printf 'ascii-scripts: %s install scripts, every byte of them ASCII.\n' "$checked"
