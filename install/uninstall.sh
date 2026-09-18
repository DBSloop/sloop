#!/bin/sh
#
# Take sloop off a Linux or macOS machine -- the binary the installer wrote, and the PATH
# entry that points at it.
#
#   curl -fsSL https://dbsloop.github.io/uninstall.sh | sh
#
# **`sloop uninstall` is the one to run first, and this says so rather than guessing.** That
# command knows what sloop built on this machine -- a PostgreSQL it downloaded, a database, a
# role, a registry -- and removes the binary and this PATH entry at the end of it. What is
# here is the fallback for a machine where the binary is gone, or broken, or was never on
# PATH in the first place: removing the file is all a script can honestly do, because a
# script cannot know which PostgreSQL was sloop's.
#
# So: state still present and a binary still able to remove it is not something this decides
# on its own. It stops and names the flag, exactly as the CLI would.
#
# **Backups are never deleted. Not by this, not by `sloop uninstall`, not ever** -- a registry
# is a minute of typing and a PostgreSQL is a download, but a dump that is gone is gone.
#
#   --dir <path>     the directory it was installed into, if not ~/.local/bin
#   --binary-only    remove the binary and the PATH entry, leave sloop's own state alone
#   --help

set -eu

DIR="${SLOOP_INSTALL_DIR:-}"
BINARY_ONLY=0

BEGIN_MARK="# >>> sloop >>>"
END_MARK="# <<< sloop <<<"

REMOVED=0

main() {
    parse_arguments "$@"
    choose_directory

    binary="$DIR/sloop"
    check_for_state "$binary"

    remove_binary "$binary"
    remove_path_entries

    say ""
    if [ "$REMOVED" -eq 0 ]; then
        say "Nothing to remove -- sloop was not installed by this installer."
    else
        say "sloop is off this machine."
        say "Open shells still have the old PATH; the next one will not."
    fi
}

parse_arguments() {
    while [ $# -gt 0 ]; do
        case "$1" in
        --dir)
            shift
            [ $# -gt 0 ] || usage_error "--dir needs a directory"
            DIR="$1"
            ;;
        --dir=*) DIR="${1#--dir=}" ;;
        --binary-only) BINARY_ONLY=1 ;;
        -h | --help)
            help_text
            exit 0
            ;;
        *) usage_error "unknown option: $1" ;;
        esac
        shift
    done
}

help_text() {
    cat <<'HELP'
Remove sloop's binary and its PATH entry.

  curl -fsSL https://dbsloop.github.io/uninstall.sh | sh

Options:
  --dir <path>     the directory sloop was installed into, if not ~/.local/bin
  --binary-only    leave sloop's database, registry and config alone
  -h, --help       this

`sloop uninstall` removes sloop's own PostgreSQL, database and registry as well, and
finishes by doing everything this script does. Run that first when the binary still works.
Neither deletes a backup.
HELP
}

choose_directory() {
    if [ -z "$DIR" ]; then
        [ -n "${HOME:-}" ] ||
            fail "HOME is not set, so there is no ~/.local/bin to look in" \
                "  name the directory with --dir <path>"
        DIR="$HOME/.local/bin"
    fi
}

# Refuse to strand sloop's own state behind a binary that could still remove it.
#
# **Rule 4's shape, in a script.** There is no terminal here to ask, so the two ways
# forward are named and the one that needs a decision is a flag: run the CLI's own
# uninstall, or say `--binary-only` and mean it.
check_for_state() {
    binary="$1"

    [ "$BINARY_ONLY" -eq 0 ] || return 0
    [ -x "$binary" ] || return 0

    # **`~/.sloop` first, because that is where the store is**, on all three platforms --
    # `cli/src/registry/locations.rs` settled that. The other two are where a store used to
    # live and may still be sitting on a machine that has not run sloop since; `adopt` moves
    # one of those the next time sloop runs, so a script that ignored them would call a
    # machine clean that is one command away from not being.
    store=""
    for candidate in \
        "${HOME:-}/.sloop" \
        "${XDG_CONFIG_HOME:-${HOME:-}/.config}/sloop" \
        "${HOME:-}/Library/Application Support/sloop"; do
        if [ -d "$candidate" ]; then
            store="$candidate"
            break
        fi
    done

    [ -n "$store" ] || return 0

    stop "sloop still has state on this machine, and $binary can still remove it:" \
        "" \
        "  $store" \
        "" \
        "  Run this first -- it removes the PostgreSQL sloop installed, its database," \
        "  its role and its registry, then the binary and the PATH entry:" \
        "" \
        "      $binary uninstall" \
        "" \
        "  Or leave that state where it is and take only the binary:" \
        "" \
        "      curl -fsSL https://dbsloop.github.io/uninstall.sh | sh -s -- --binary-only" \
        "" \
        "  Backups are not deleted by either."
}

# **Never the directory on Unix.** `~/.local/bin` is somebody's own bin directory with
# somebody's own programs in it, so only the one file sloop put there goes -- and an empty one
# that sloop created is left as well, because an empty directory harms nothing and guessing
# wrong about who made it does.
remove_binary() {
    binary="$1"

    if [ -e "$binary" ] || [ -L "$binary" ]; then
        rm -f "$binary" || fail "could not remove $binary"
        say "  removed $binary"
        REMOVED=1
    fi
}

# Take the block out of every startup file that could be holding one.
#
# **Every candidate, not just the current shell's.** Somebody who installed under bash and
# has since moved to zsh has the block in `.bashrc`, and a script that only looks at
# `$SHELL` would leave it there pointing at a binary that no longer exists.
remove_path_entries() {
    for rc in \
        "${HOME:-}/.bashrc" \
        "${HOME:-}/.bash_profile" \
        "${HOME:-}/.bash_login" \
        "${HOME:-}/.profile" \
        "${ZDOTDIR:-${HOME:-}}/.zshrc" \
        "${HOME:-}/.kshrc"; do
        strip_block "$rc"
    done

    fish_conf="${XDG_CONFIG_HOME:-${HOME:-}/.config}/fish/conf.d/sloop.fish"
    if [ -f "$fish_conf" ]; then
        rm -f "$fish_conf" || fail "could not remove $fish_conf"
        say "  removed $fish_conf"
        REMOVED=1
    fi
}

# Remove the marked block from one file, leaving everything else byte for byte.
#
# **A temporary file beside the original, then a rename.** Editing in place would truncate
# somebody's `.bashrc` if the machine lost power halfway; a rename either happens or does not.
#
# **The temporary starts as a copy, which is how the mode survives.** `cp -p` carries the
# permissions across and `>` then truncates the file it has already created without changing
# them -- so a `.profile` that was `600` is still `600` afterwards. The alternative is reading
# the mode back out of `ls`, which is a different answer on BSD and on GNU.
strip_block() {
    rc="$1"

    [ -f "$rc" ] || return 0
    grep -Fq "$BEGIN_MARK" "$rc" 2>/dev/null || return 0

    tmp="$rc.sloop-uninstall.$$"
    cp -p "$rc" "$tmp" || fail "could not write beside $rc"

    awk -v begin="$BEGIN_MARK" -v end="$END_MARK" '
        $0 == begin { inside = 1; next }
        $0 == end   { inside = 0; next }
        !inside     { print }
    ' "$rc" >"$tmp" || {
        rm -f "$tmp"
        fail "could not rewrite $rc"
    }

    mv -f "$tmp" "$rc" || {
        rm -f "$tmp"
        fail "could not rewrite $rc"
    }

    say "  removed sloop's PATH block from $rc"
    REMOVED=1
}

say() {
    printf '%s\n' "$*"
}

fail() {
    printf '\n  sloop uninstall failed.\n\n' >&2
    for line in "$@"; do
        printf '  %s\n' "$line" >&2
    done
    printf '\n' >&2
    exit 1
}

# The one thing this script cannot decide on its own, said the way the CLI says it: what is
# in the way, and the flag that answers it. Exit 2 is sloop's own code for a question a run
# could not be asked.
stop() {
    printf '\n  sloop uninstall stopped.\n\n' >&2
    for line in "$@"; do
        printf '  %s\n' "$line" >&2
    done
    printf '\n' >&2
    exit 2
}

usage_error() {
    printf '\n  %s\n\n' "$1" >&2
    help_text >&2
    exit 2
}

main "$@"
