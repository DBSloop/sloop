#!/bin/sh
#
# Check that every PostgreSQL build sloop is prepared to install is still the build it was
# pinned to, and say so when PostgreSQL has moved on.
#
# `cli/src/tools/releases.rs` holds a size and a SHA-256 for each archive sloop will fetch
# on Windows, and refuses to install anything that does not match. That is only worth
# anything if the pin is right, and a pin nobody re-checks goes wrong in two directions:
# the vendor can replace an archive under the same name, and PostgreSQL can release a major
# that no row covers. Neither shows up in a compiler error, so it shows up here.
#
# A hash that has moved is a hard failure. It means the file sloop would download is not
# the file whose contents were looked at, which is exactly the thing the checksum exists to
# catch.
#
# A supported major with no pin is also a failure, because the alternative is a warning
# that nobody reads until somebody with PostgreSQL 19 finds out the hard way. Adding a row
# is one line and one run of this script.
#
# Not on every push: each row is about a third of a gigabyte, and re-downloading that on
# every commit would be rude to a server that is doing us a favour. Weekly, and whenever
# `releases.rs` itself changes.
#
#   usage: sh ci/pinned-releases.sh [path/to/releases.rs]

set -eu

source_file="${1:-$(dirname "$0")/../cli/src/tools/releases.rs}"
index_url='https://www.postgresql.org/versions.json'

if [ ! -f "$source_file" ]; then
    printf 'pinned-releases: no source at %s\n' "$source_file" >&2
    exit 1
fi

work=$(mktemp -d)
# shellcheck disable=SC2064
trap "rm -rf '$work'" EXIT INT TERM

# The table is a list of plain literals, so it reads cleanly with awk: collect the fields
# of each `Release { ... }` and print one line per row.
pins=$(
    awk '
        /^pub const PINNED/  { inside = 1 }
        inside && /major:/   { major   = value($0) }
        inside && /minor:/   { minor   = value($0) }
        inside && /build:/   { build   = value($0) }
        inside && /bytes:/   { bytes   = value($0); gsub(/_/, "", bytes) }
        inside && /sha256:/  { sha     = value($0); gsub(/"/, "", sha)
                               print major, minor, build, bytes, sha }
        inside && /^\];/     { inside = 0 }
        function value(line,   field) {
            field = substr(line, index(line, ":") + 1)
            gsub(/^[ \t]+|[ \t]*,[ \t]*$|[ \t]+$/, "", field)
            return field
        }
    ' "$source_file"
)

if [ -z "$pins" ]; then
    printf 'pinned-releases: no releases found in %s\n' "$source_file" >&2
    exit 1
fi

failed=0
covered=''

printf '%s\n' "$pins" | while IFS=' ' read -r major minor build bytes sha; do
    name="postgresql-${major}.${minor}-${build}-windows-x64-binaries.zip"
    url="https://get.enterprisedb.com/postgresql/${name}"

    printf 'pinned-releases: checking %s\n' "$name"
    if ! curl --fail --location --proto '=https' --proto-redir '=https' \
        --silent --show-error --max-time 1800 --output "$work/$name" "$url"; then
        printf '  could not download %s\n' "$url" >&2
        exit 1
    fi

    got_bytes=$(wc -c < "$work/$name" | tr -d ' ')
    got_sha=$(sha256sum "$work/$name" | cut -d' ' -f1)
    rm -f "$work/$name"

    if [ "$got_bytes" != "$bytes" ] || [ "$got_sha" != "$sha" ]; then
        {
            echo
            echo "  $name is not what it was pinned to."
            echo "    pinned  $bytes bytes  $sha"
            echo "    served  $got_bytes bytes  $got_sha"
            echo
            echo '  Either the archive was replaced under the same name, or something is'
            echo '  answering for it that should not be. Look at the file before touching'
            echo '  the pin: sloop refuses to install anything that does not match, and'
            echo '  that refusal is the whole point of the row.'
            echo
        } >&2
        exit 1
    fi

    printf '  ok — %s bytes, sha256 matches\n' "$got_bytes"
done || failed=1

[ "$failed" -eq 0 ] || exit 1

# Now the other direction: a supported major that no row covers.
if ! curl --fail --location --proto '=https' --silent --show-error --max-time 60 \
    --output "$work/versions.json" "$index_url"; then
    printf 'pinned-releases: could not read %s, so coverage was not checked\n' "$index_url" >&2
    exit 1
fi

newest_supported=$(
    tr '{' '\n' < "$work/versions.json" \
    | grep '"supported": true' \
    | sed -n 's/.*"major": "\([0-9]*\).*/\1/p' \
    | sort -n | tail -1
)

newest_pinned=$(printf '%s\n' "$pins" | awk '{ print $1 }' | sort -n | tail -1)
covered=$(printf '%s\n' "$pins" | awk '{ print $1 }' | sort -n | tr '\n' ' ')

if [ -z "$newest_supported" ]; then
    printf 'pinned-releases: the index said nothing useful, so coverage was not checked\n' >&2
    exit 1
fi

if [ "$newest_supported" -gt "$newest_pinned" ]; then
    {
        echo
        echo "  PostgreSQL $newest_supported is supported and nothing here installs it."
        echo "  Pinned majors: $covered"
        echo
        echo '  A pinned pg_dump only dumps servers up to its own major, so until a row'
        echo "  for $newest_supported exists, anybody sloop installs tools for cannot back up a"
        echo "  $newest_supported server. Add one to cli/src/tools/releases.rs:"
        echo
        echo '    1. find the newest build:'
        echo '         curl -sI https://get.enterprisedb.com/postgresql/postgresql-'"$newest_supported"'.N-B-windows-x64-binaries.zip'
        echo '    2. download it, take its size and sha256sum'
        echo '    3. add the row, and run this script again'
        echo
    } >&2
    exit 1
fi

printf 'pinned-releases: every pin still matches, and %s covers the newest supported major.\n' \
    "$newest_pinned"
