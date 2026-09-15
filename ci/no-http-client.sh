#!/bin/sh
#
# Fail the build if an HTTP client has appeared in the binary's dependency graph.
#
# The README, the docs and the landing page all make the same promise: `cargo tree` on
# this repo shows no HTTP client, so there is nothing in the binary that *could* send a
# credential anywhere. That is a claim about a dependency graph, and a claim about a
# dependency graph that nothing checks is a claim that survives exactly until the first
# convenient `cargo add`. So it is checked, on every push, and it fails the build.
#
# Normal, build and dev edges are all checked. A build-dependency that can open a socket
# phones home from CI rather than from the user's machine, which is not better.
#
#   usage: sh ci/no-http-client.sh [path/to/Cargo.toml]

set -eu

manifest="${1:-$(dirname "$0")/../cli/Cargo.toml}"

if [ ! -f "$manifest" ]; then
    printf 'no-http-client: no manifest at %s\n' "$manifest" >&2
    exit 1
fi

edges='normal,build,dev'

# `--target all` matters: a dependency behind `[target.'cfg(windows)'.dependencies]` is
# invisible to a default `cargo tree` run on Linux, and that is precisely where something
# would hide.
crates=$(
    cargo tree \
        --manifest-path "$manifest" \
        --edges "$edges" \
        --all-features \
        --target all \
        --prefix none \
    | awk 'NF { print $1 }' \
    | sort -u
)

# Families, because the offender is rarely the crate you would have typed: `reqwest`
# arrives as `hyper`, `hyper-util` and `h2` long before anyone notices.
denied=''
for crate in $crates; do
    case "$crate" in
    reqwest | reqwest-* | \
    hyper | hyper-* | h2 | h3 | h3-* | \
    ureq | ureq-* | \
    isahc | curl | curl-sys | \
    attohttpc | minreq | http-req | \
    surf | awc | \
    actix-web | actix-http | \
    axum | axum-* | warp | tide | rouille | tiny_http | \
    rocket | rocket_*)
        denied="$denied $crate"
        ;;
    esac
done

count=$(printf '%s\n' "$crates" | wc -l | tr -d ' ')

if [ -n "$denied" ]; then
    {
        echo
        echo '  The guarantee is broken.'
        echo
        echo '  "Your credentials never leave your machine" is not a slogan on this'
        echo '  project, it is a property of the dependency graph, and an HTTP client'
        echo '  has just entered it:'
        echo
        for crate in $denied; do
            printf '    %s\n' "$crate"
            cargo tree \
                --manifest-path "$manifest" \
                --edges "$edges" \
                --target all \
                --invert "$crate" 2>/dev/null \
                | sed 's/^/        /' || true
            echo
        done
        echo '  Remove it, or shell out to the system curl / Invoke-WebRequest the way'
        echo '  the client-tool download does. Nothing in this binary opens a socket.'
        echo
    } >&2
    exit 1
fi

printf 'no-http-client: %s crates in the graph, not one of them speaks HTTP.\n' "$count"
