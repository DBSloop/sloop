#!/bin/sh
#
# Install sloop on Linux or macOS.
#
#   curl -fsSL https://dbsloop.github.io/install.sh | sh
#
# **It never asks anything, and that is structural rather than polite.** The command above
# pipes this file into `sh`, so standard input *is* the script -- a `read` here would eat the
# rest of itself, and rule 4 forbids a prompt without a terminal anyway. Every decision is
# therefore made from the machine or from a flag, and anything that cannot be decided that
# way fails with the flag that would have decided it.
#
# **The binary it downloads holds no network code, and neither does this.** sloop reaches the
# network exactly once -- here, through the system's own `curl` or `wget`, before the binary
# exists. What it fetches is proved against `SHA256SUMS` from the same release before a byte
# of it is unpacked, which is the same rule `ci/no-http-client.sh` guards on the other side.
#
#   --version <x.y.z>   a release other than the latest
#   --dir <path>        somewhere other than ~/.local/bin
#   --from <url|dir>    somewhere other than GitHub releases -- the installer's own tests
#   --no-modify-path    install, and leave the shell rc alone
#   --help
#
# The same four also arrive as SLOOP_VERSION, SLOOP_INSTALL_DIR, SLOOP_INSTALL_BASE and
# SLOOP_NO_MODIFY_PATH, because `curl ... | sh` cannot pass an argument without `-s --`.

set -eu

REPO_URL="https://github.com/DBSloop/sloop"
RELEASES_URL="$REPO_URL/releases"

VERSION="${SLOOP_VERSION:-}"
BASE="${SLOOP_INSTALL_BASE:-}"
DIR="${SLOOP_INSTALL_DIR:-}"
MODIFY_PATH=1
if [ -n "${SLOOP_NO_MODIFY_PATH:-}" ]; then
    MODIFY_PATH=0
fi

# The block written into a shell rc, and the two lines that let it be taken out again
# exactly. A marked block is removable by a machine; a bare `export PATH=...` appended to
# somebody's `.bashrc` is removable only by a person reading it.
BEGIN_MARK="# >>> sloop >>>"
END_MARK="# <<< sloop <<<"

TMP=""

main() {
    parse_arguments "$@"
    detect_target
    choose_directory
    choose_base

    say "sloop $(printf '%s' "${VERSION:-latest}") for $TARGET"

    TMP=$(mktemp -d 2>/dev/null || mktemp -d -t sloop) ||
        fail "could not make a temporary directory"
    trap 'rm -rf "$TMP"' EXIT INT TERM

    get "$BASE/$ARCHIVE" "$TMP/$ARCHIVE"
    get "$BASE/SHA256SUMS" "$TMP/SHA256SUMS"
    verify "$TMP/$ARCHIVE" "$ARCHIVE" "$TMP/SHA256SUMS"
    unpack "$TMP/$ARCHIVE" "$TMP/unpacked"

    install_binary "$TMP/unpacked/$BINARY" "$DIR/$BINARY"

    if [ "$MODIFY_PATH" -eq 1 ]; then
        put_on_path "$DIR"
    else
        RC_WRITTEN=""
        ALREADY_ON_PATH=0
        say "  PATH untouched, as asked"
    fi

    finish
}

# ---------------------------------------------------------------------------- arguments

parse_arguments() {
    while [ $# -gt 0 ]; do
        case "$1" in
        --version)
            shift
            [ $# -gt 0 ] || usage_error "--version needs a release, like --version 1.2.3"
            VERSION="$1"
            ;;
        --version=*) VERSION="${1#--version=}" ;;
        --dir)
            shift
            [ $# -gt 0 ] || usage_error "--dir needs a directory"
            DIR="$1"
            ;;
        --dir=*) DIR="${1#--dir=}" ;;
        --from)
            shift
            [ $# -gt 0 ] || usage_error "--from needs a URL or a directory"
            BASE="$1"
            ;;
        --from=*) BASE="${1#--from=}" ;;
        --no-modify-path) MODIFY_PATH=0 ;;
        -h | --help)
            help_text
            exit 0
            ;;
        *) usage_error "unknown option: $1" ;;
        esac
        shift
    done

    # A leading `v` is how the tag is written and how a person will type it; the asset
    # index is written without one. Accept both rather than fail on the obvious spelling.
    VERSION="${VERSION#v}"
}

help_text() {
    cat <<'HELP'
Install sloop -- a database operations CLI.

  curl -fsSL https://dbsloop.github.io/install.sh | sh

Options:
  --version <x.y.z>   install that release instead of the latest
  --dir <path>        install into that directory instead of ~/.local/bin
  --from <url|dir>    fetch from there instead of GitHub releases
  --no-modify-path    do not touch any shell startup file
  -h, --help          this

Passing an option through the pipe needs sh to be told where its own arguments stop:

  curl -fsSL https://dbsloop.github.io/install.sh | sh -s -- --version 1.2.3
HELP
}

# ------------------------------------------------------------------------------- machine

# Which release archive this machine takes.
#
# **Named as Rust names it**, because the target triple is what the release workflow builds
# with and what a person reading the assets page will recognise. A machine sloop has no
# build for is told so, with the page that lists what there is -- never a guess at the
# nearest one, which would install a binary that cannot run.
detect_target() {
    kernel=$(uname -s 2>/dev/null || echo unknown)
    machine=$(uname -m 2>/dev/null || echo unknown)

    case "$kernel" in
    Linux) os="unknown-linux-musl" ;;
    Darwin) os="apple-darwin" ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT)
        fail "this is Windows -- run the PowerShell installer instead:" \
            "  irm https://dbsloop.github.io/install.ps1 | iex"
        ;;
    *)
        fail "sloop has no build for $kernel" \
            "  the releases page lists what there is: $RELEASES_URL"
        ;;
    esac

    case "$machine" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64) arch="aarch64" ;;
    *)
        fail "sloop has no build for $machine" \
            "  the releases page lists what there is: $RELEASES_URL"
        ;;
    esac

    # **Rosetta lies, and the lie installs the wrong binary.** A shell running under
    # Rosetta 2 on Apple silicon reports `x86_64` from `uname -m`; the machine is aarch64
    # and should have the native build. macOS says which it is if it is asked directly.
    if [ "$kernel" = "Darwin" ] && [ "$arch" = "x86_64" ] &&
        [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
        arch="aarch64"
    fi

    TARGET="$arch-$os"
    BINARY="sloop"
    ARCHIVE="sloop-$TARGET.tar.gz"
}

choose_directory() {
    if [ -z "$DIR" ]; then
        [ -n "${HOME:-}" ] || fail "HOME is not set, so there is no ~/.local/bin to install into" \
            "  name one with --dir <path>"
        DIR="$HOME/.local/bin"
    fi
}

# Where the archive and its checksums come from.
#
# `latest/download` is a redirect GitHub serves itself, so the newest release needs no API
# call, no JSON and no token -- which matters on a machine that has `curl` and nothing else.
choose_base() {
    if [ -n "$BASE" ]; then
        return
    fi
    if [ -n "$VERSION" ]; then
        BASE="$RELEASES_URL/download/v$VERSION"
    else
        BASE="$RELEASES_URL/latest/download"
    fi
}

# ------------------------------------------------------------------------------ fetching

# Fetch one file, from a URL or from a directory.
#
# A directory is what the installer's own tests point at: the release layout built locally,
# so the whole path below -- checksum, unpack, install, PATH -- is exercised on a runner with
# no release to download.
get() {
    from="$1"
    to="$2"

    case "$from" in
    http://* | https://*)
        if have curl; then
            curl -fsSL "$from" -o "$to" ||
                fail "could not download $from"
        elif have wget; then
            wget -q "$from" -O "$to" || fail "could not download $from"
        else
            fail "neither curl nor wget is on this machine, and one of them has to be" \
                "  install either, or download by hand from $RELEASES_URL"
        fi
        ;;
    file://*)
        cp "${from#file://}" "$to" || fail "could not read $from"
        ;;
    *)
        cp "$from" "$to" || fail "could not read $from"
        ;;
    esac
}

# Prove the archive is the one the release published, before anything is unpacked.
#
# **Three hash programs, because the three platforms ship different ones** -- `sha256sum` on
# Linux, `shasum` on macOS, `openssl` on whatever has neither. A machine with none of them
# is told, and the install stops: an unverified download is not a faster install, it is a
# different product.
verify() {
    file="$1"
    name="$2"
    sums="$3"

    # The trailing `\r` comes off before the comparison: a SHA256SUMS written on Windows has
    # CRLF endings, and a filename with an invisible carriage return on the end matches
    # nothing and fails with a message about the release not carrying this build.
    expected=$(
        awk -v want="$name" '
            { candidate = $2
              sub(/\r$/, "", candidate)
              sub(/^\*/, "", candidate)
              if (candidate == want) { print tolower($1); exit } }
        ' "$sums"
    )
    [ -n "$expected" ] ||
        fail "SHA256SUMS from that release does not mention $name" \
            "  the release may not carry a build for $TARGET -- see $RELEASES_URL"

    if have sha256sum; then
        actual=$(sha256sum "$file" | awk '{ print tolower($1) }')
    elif have shasum; then
        actual=$(shasum -a 256 "$file" | awk '{ print tolower($1) }')
    elif have openssl; then
        actual=$(openssl dgst -sha256 "$file" | awk '{ print tolower($NF) }')
    else
        fail "no sha256sum, shasum or openssl on this machine, so the download cannot be proved" \
            "  install one of them -- sloop does not unpack an archive it has not checked"
    fi

    if [ "$actual" != "$expected" ]; then
        fail "$name is not what that release published" \
            "  expected $expected" \
            "  received $actual" \
            "  nothing has been installed. Try again; if it happens twice, say so at $REPO_URL/issues"
    fi

    say "  checksum verified"
}

unpack() {
    archive="$1"
    into="$2"

    mkdir -p "$into"
    have tar || fail "tar is not on this machine, and the release archives are tarballs"
    tar -xzf "$archive" -C "$into" || fail "could not unpack $archive"

    [ -f "$into/$BINARY" ] ||
        fail "$archive does not hold a $BINARY binary" \
            "  download by hand from $RELEASES_URL"
}

# ----------------------------------------------------------------------------- installing

# Put the binary where it goes.
#
# **`mv` over the old one, never `cp` into it.** Replacing the file by rename swaps an
# inode; writing into the file in place rewrites a binary that a running sloop is still
# reading, and on some systems that is a crash rather than an upgrade.
install_binary() {
    from="$1"
    to="$2"

    mkdir -p "$DIR" || fail "could not create $DIR"
    chmod +x "$from"

    if ! mv -f "$from" "$to" 2>/dev/null; then
        # Across filesystems `mv` falls back to copy-and-unlink itself, so a failure here
        # is a permission problem rather than a device boundary.
        fail "could not write $to" \
            "  install somewhere writable with --dir <path>"
    fi

    say "  installed $to"

    VERSION_LINE=$("$to" --version 2>/dev/null || echo "")
    [ -n "$VERSION_LINE" ] ||
        fail "$to does not run on this machine" \
            "  the archive for $TARGET was installed -- if that is the wrong architecture," \
            "  the releases page lists the rest: $RELEASES_URL"
}

# --------------------------------------------------------------------------------- PATH

# Put the install directory on PATH for every shell started from now on.
#
# **One marked block in one file, chosen by the shell the user actually logs into.** Which
# file that is differs by shell and by platform, and getting it wrong means an installer
# that reports success and a `sloop: command not found` in the next terminal.
put_on_path() {
    dir="$1"
    RC_WRITTEN=""
    ALREADY_ON_PATH=0

    case ":${PATH:-}:" in
    *":$dir:"*)
        ALREADY_ON_PATH=1
        say "  $dir is already on PATH"
        return
        ;;
    esac

    pick_rc

    if [ -z "$RC" ]; then
        say "  PATH not changed -- no startup file for ${SHELL_NAME:-this shell} could be found"
        return
    fi

    if [ -f "$RC" ] && grep -Fq "$BEGIN_MARK" "$RC" 2>/dev/null; then
        say "  $RC already has sloop's PATH block"
        RC_WRITTEN="$RC"
        return
    fi

    mkdir -p "$(dirname "$RC")" || fail "could not create $(dirname "$RC")"

    # Written with `$HOME` rather than the expanded path when it is under the home
    # directory, so the line keeps working on a machine where that is spelled differently --
    # a mounted home, a different username, a restored backup.
    written="$dir"
    if [ -n "${HOME:-}" ]; then
        case "$dir" in
        "$HOME"/*) written="\$HOME/${dir#"$HOME"/}" ;;
        esac
    fi

    if [ "${SHELL_NAME:-}" = "fish" ]; then
        # fish is not a POSIX shell and a `case` block is a syntax error in it. Its own
        # conf.d is a directory of files, so sloop's is a file -- which makes removing it
        # a delete rather than an edit.
        {
            echo "$BEGIN_MARK"
            echo "# Added by the sloop installer. \`sloop uninstall\` removes this file."
            echo "fish_add_path $written"
            echo "$END_MARK"
        } >>"$RC" || fail "could not write $RC"
    else
        {
            echo ""
            echo "$BEGIN_MARK"
            echo "# Added by the sloop installer. \`sloop uninstall\` removes this block."
            echo "case \":\$PATH:\" in"
            echo "    *\":$written:\"*) ;;"
            echo "    *) PATH=\"$written:\$PATH\" ;;"
            echo "esac"
            echo "export PATH"
            echo "$END_MARK"
        } >>"$RC" || fail "could not write $RC"
    fi

    RC_WRITTEN="$RC"
    say "  PATH entry added to $RC"
}

# The startup file the user's login shell actually reads.
#
# **bash on macOS is the trap.** Terminal.app starts a login shell, and a login bash reads
# the *first* of `.bash_profile`, `.bash_login`, `.profile` and no other -- so writing
# `.bashrc` there is writing to a file nothing opens, and creating a `.bash_profile` on a
# machine that keeps its settings in `.profile` silently stops bash reading `.profile` at
# all. So: the existing file wins, and a new one is only created when there is nothing.
pick_rc() {
    SHELL_NAME=$(basename "${SHELL:-/bin/sh}")
    RC=""

    case "$SHELL_NAME" in
    bash)
        if [ "$(uname -s 2>/dev/null || echo unknown)" = "Darwin" ]; then
            if [ -f "$HOME/.bash_profile" ]; then
                RC="$HOME/.bash_profile"
            elif [ -f "$HOME/.bash_login" ]; then
                RC="$HOME/.bash_login"
            elif [ -f "$HOME/.profile" ]; then
                RC="$HOME/.profile"
            else
                RC="$HOME/.bash_profile"
            fi
        else
            RC="$HOME/.bashrc"
        fi
        ;;
    zsh) RC="${ZDOTDIR:-$HOME}/.zshrc" ;;
    fish) RC="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/sloop.fish" ;;
    ksh | mksh | ksh93) RC="$HOME/.kshrc" ;;
    *) RC="$HOME/.profile" ;;
    esac
}

# ----------------------------------------------------------------------------------- end

# The last thing a person reads, and the one line they have to act on.
#
# **The restart is said plainly and last.** A shell reads its startup file once, when it
# starts; the terminal this ran in has already read its own. Telling somebody that after
# they have hit `sloop: command not found` is telling them too late.
finish() {
    say ""
    say "$VERSION_LINE is installed."
    say ""
    if [ -n "${RC_WRITTEN:-}" ]; then
        say "Restart your shell, or start a new terminal, before running sloop."
        say "A shell reads $RC_WRITTEN once, when it starts -- this one has already read it."
        say ""
        say "To use it in this shell without restarting:"
        say "    export PATH=\"$DIR:\$PATH\""
    elif [ "${ALREADY_ON_PATH:-0}" -eq 1 ]; then
        say "Run: sloop"
    else
        say "Run: $DIR/sloop"
    fi
    say ""
    say "Then: sloop setup"
}

# ------------------------------------------------------------------------------- helpers

have() {
    command -v "$1" >/dev/null 2>&1
}

say() {
    printf '%s\n' "$*"
}

fail() {
    printf '\n  sloop install failed.\n\n' >&2
    for line in "$@"; do
        printf '  %s\n' "$line" >&2
    done
    printf '\n' >&2
    exit 1
}

usage_error() {
    printf '\n  %s\n\n' "$1" >&2
    help_text >&2
    exit 2
}

main "$@"
