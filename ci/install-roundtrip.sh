#!/bin/sh
#
# Install sloop the way a stranger does, then take it off again, and fail the build if
# either half leaves something behind.
#
# **Rule 14 is why this exists.** An installer is the one piece of a release nobody runs
# until a stranger runs it, and "we tested it by hand once" is how a release ships a broken
# one. So it is driven here: a release layout is built from the binary this commit produced,
# `install.sh` is pointed at it, and the three things `R21` promises are asserted --
#
#   the binary lands and runs
#   a new shell resolves `sloop` without being told where it is
#   uninstall leaves nothing behind, the PATH entry included
#
# -- followed by the failure paths, because an installer that cannot be trusted to refuse is
# an installer that cannot be trusted to accept: a tampered archive, a release with no build
# for this machine, and a flag nobody knows.
#
# **Nothing here touches the machine it runs on.** The install goes into a temporary
# directory with a temporary HOME, so the startup file it writes is one this script made and
# deletes.
#
#   usage: sh ci/install-roundtrip.sh [path/to/sloop]

set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
binary="${1:-$root/cli/target/debug/sloop}"

if [ ! -x "$binary" ]; then
    printf 'install-roundtrip: no binary at %s\n' "$binary" >&2
    printf '  build one first: cargo build --manifest-path cli/Cargo.toml --bin sloop\n' >&2
    exit 1
fi

# The target triple, worked out a second time rather than asked of `install.sh`. Two
# implementations that agree is a check; one implementation agreeing with itself is not.
case "$(uname -s)" in
Linux) os="unknown-linux-musl" ;;
Darwin) os="apple-darwin" ;;
*)
    printf 'install-roundtrip: this script is for Linux and macOS\n' >&2
    exit 1
    ;;
esac
case "$(uname -m)" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="aarch64" ;;
*)
    printf 'install-roundtrip: no sloop build for %s\n' "$(uname -m)" >&2
    exit 1
    ;;
esac
target="$arch-$os"
archive="sloop-$target.tar.gz"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

# ------------------------------------------------------------------------------- checking

checks=0
passed=0
failed=0

# **Every assertion runs inside an `if`**, which is what keeps `set -eu` above from turning
# a failed check into an exit before it can be counted -- and a run that stops at the first
# failure is a run that only ever reports one.
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
    if [ "$2" = "$3" ]; then record 0 "$1"; else
        record 1 "$1 -- expected [$3], got [$2]"
    fi
}

check_exit() {
    label="$1"
    want="$2"
    shift 2
    got=0
    "$@" >/dev/null 2>&1 || got=$?
    check_same "$label" "$got" "$want"
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    else
        shasum -a 256 "$1" | awk '{ print $1 }'
    fi
}

# What `command -v sloop` says in a *new* shell of the kind this platform actually starts --
# which is the thing `R21` promises, and the reason `install.sh` picks the file it does.
#
# **The two platforms need two shells, and that asymmetry is the whole point.** A terminal on
# Linux starts a non-login interactive bash, which reads `~/.bashrc`; Terminal.app on macOS
# starts a *login* bash, which reads `~/.bash_profile` and never looks at `.bashrc`. Asking
# the wrong one here would pass against an installer that writes to a file nothing opens.
resolves_to() {
    if [ "$(uname -s)" = "Darwin" ]; then
        HOME="$1" bash -lic 'command -v sloop' 2>/dev/null | tr -d '\r' | tail -n 1
    else
        HOME="$1" bash -ic 'command -v sloop' 2>/dev/null | tr -d '\r' | tail -n 1
    fi
}

# ------------------------------------------------------- a release, built the way R22 will

release="$work/release"
staging="$work/staging"
home="$work/home"
bin="$work/bin"
mkdir -p "$release" "$staging" "$home" "$bin"

cp "$binary" "$staging/sloop"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$staging/"
tar -czf "$release/$archive" -C "$staging" sloop LICENSE-MIT LICENSE-APACHE
(cd "$release" && printf '%s  %s\n' "$(sha256 "$archive")" "$archive" >SHA256SUMS)

printf 'install-roundtrip: %s, from a release laid out in %s\n\n' "$target" "$release"

# ----------------------------------------------------------------------------- installing

if ! HOME="$home" SHELL="/bin/bash" sh "$root/install/install.sh" \
    --from "$release" --dir "$bin" >"$work/install.log" 2>&1; then
    sed 's/^/  | /' "$work/install.log" >&2
    printf 'install-roundtrip: install.sh failed\n' >&2
    exit 1
fi
sed 's/^/  | /' "$work/install.log"
printf '\n'

# The startup file it says it wrote, taken from what it printed rather than assumed: which
# file that is differs by platform, and an assertion that guessed would be asserting the
# guess.
rc=$(sed -n 's/^ *PATH entry added to //p' "$work/install.log" | tail -n 1)

check "the binary is installed and executable" test -x "$bin/sloop"
check "the installed binary runs" "$bin/sloop" --version
check "it names the startup file it wrote" test -n "$rc"
check "the PATH block is in the startup file it named" grep -Fq '# >>> sloop >>>' "$rc"
check "the block names the directory it installed into" grep -Fq "$bin" "$rc"
check "it says the shell has to be restarted" grep -Fq 'Restart your shell' "$work/install.log"

check_same "a new shell resolves sloop" "$(resolves_to "$home")" "$bin/sloop"

# Running it twice is running it once: an upgrade must not append a second block.
HOME="$home" SHELL="/bin/bash" sh "$root/install/install.sh" \
    --from "$release" --dir "$bin" >"$work/again.log" 2>&1
check_same "installing twice leaves one PATH block, not two" \
    "$(grep -c '>>> sloop >>>' "$rc")" "1"

# --------------------------------------------------------------------------- uninstalling

# **Under the same HOME the install used.** `uninstall.sh` finds the startup files through
# `$HOME`, so running it with the runner's own would be asking it about the wrong machine --
# and would pass while leaving the block exactly where it was.
if ! HOME="$home" sh "$root/install/uninstall.sh" --dir "$bin" >"$work/uninstall.log" 2>&1; then
    sed 's/^/  | /' "$work/uninstall.log" >&2
    printf 'install-roundtrip: uninstall.sh failed\n' >&2
    exit 1
fi
sed 's/^/  | /' "$work/uninstall.log"
printf '\n'

check_not "the binary is gone" test -e "$bin/sloop"
check_not "not one mention of sloop is left in the startup file" grep -Fq 'sloop' "$rc"
check_same "a new shell no longer resolves sloop" "$(resolves_to "$home")" ""

check "uninstalling twice is not an error" \
    sh -c "HOME='$home' sh '$root/install/uninstall.sh' --dir '$bin'"
check "the second uninstall says there was nothing to remove" \
    sh -c "HOME='$home' sh '$root/install/uninstall.sh' --dir '$bin' | grep -Fq 'Nothing to remove'"

# ------------------------------------------------------------------------- the refusals

# A tampered archive. The checksum is the only thing standing between a release and whatever
# was actually served, so an installer that unpacks one it did not check has no checksum.
tampered="$work/tampered"
mkdir -p "$tampered/bin"
cp "$release/SHA256SUMS" "$tampered/"
printf 'not the release you were looking for' | gzip >"$tampered/$archive"

check_not "a tampered archive is refused" \
    sh -c "HOME='$tampered' sh '$root/install/install.sh' --from '$tampered' --dir '$tampered/bin' \
        >'$work/tampered.log' 2>&1"
check_not "nothing is installed when the checksum does not match" test -e "$tampered/bin/sloop"
check "the refusal says why" grep -Fq 'not what that release published' "$work/tampered.log"

# A release that does not carry a build for this machine. The clear message matters: the
# alternative is a 404 from curl and a person who does not know what it means.
absent="$work/absent"
mkdir -p "$absent"
cp "$release/$archive" "$absent/$archive"
: >"$absent/SHA256SUMS"

check_not "a release with no checksum for this target is refused" \
    sh -c "HOME='$absent' sh '$root/install/install.sh' --from '$absent' --dir '$absent/bin' \
        >'$work/absent.log' 2>&1"
check "it names the missing entry rather than a 404" grep -Fq 'does not mention' "$work/absent.log"

# --no-modify-path means what it says.
quiet="$work/quiet"
mkdir -p "$quiet"
HOME="$quiet" SHELL="/bin/bash" sh "$root/install/install.sh" \
    --from "$release" --dir "$quiet/bin" --no-modify-path >"$work/quiet.log" 2>&1
check "--no-modify-path installs" test -x "$quiet/bin/sloop"
check_not "--no-modify-path writes no startup file at all" \
    sh -c "ls -A '$quiet' | grep -q '^\\.'"

# An unknown flag is a usage error, and exits 2 the way sloop itself does.
check_exit "an unknown option exits 2" 2 sh "$root/install/install.sh" --wat

# ------------------------------------------------------------------- the other two shells

# zsh and fish are not bash, and the file each of them reads is not `.bashrc`. An installer
# that wrote one file for everybody would pass every check above and still leave a zsh user
# with `command not found`.
zsh_home="$work/zsh"
mkdir -p "$zsh_home"
HOME="$zsh_home" SHELL="/usr/bin/zsh" sh "$root/install/install.sh" \
    --from "$release" --dir "$zsh_home/bin" >"$work/zsh.log" 2>&1
check "a zsh login gets its block in .zshrc" grep -Fq '>>> sloop >>>' "$zsh_home/.zshrc"

# fish is not a POSIX shell: a `case` block is a syntax error in it, so what it gets is a
# file of its own in `conf.d` using its own command.
fish_home="$work/fish"
mkdir -p "$fish_home"
HOME="$fish_home" SHELL="/usr/bin/fish" sh "$root/install/install.sh" \
    --from "$release" --dir "$fish_home/bin" >"$work/fish.log" 2>&1
check "a fish login gets a conf.d file, not a case block" \
    grep -Fq 'fish_add_path' "$fish_home/.config/fish/conf.d/sloop.fish"
check_not "and no case statement fish cannot parse" \
    grep -Fq 'case ' "$fish_home/.config/fish/conf.d/sloop.fish"

HOME="$fish_home" sh "$root/install/uninstall.sh" --dir "$fish_home/bin" >/dev/null 2>&1
check_not "uninstall takes the fish file with it" \
    test -e "$fish_home/.config/fish/conf.d/sloop.fish"

# ----------------------------------------------------------------- state in the way

# **Rule 4's shape, in a script.** `sloop uninstall` is the one that knows what sloop built;
# a script that removed the binary first would strand a PostgreSQL behind a command that can
# no longer be run. With no terminal to ask, it names the flag and exits 2.
stateful="$work/stateful"
mkdir -p "$stateful/.config/sloop" "$stateful/bin"
cp "$binary" "$stateful/bin/sloop"
check_exit "uninstall stops when sloop's own state is still there" 2 \
    sh -c "HOME='$stateful' sh '$root/install/uninstall.sh' --dir '$stateful/bin' \
        >'$work/stateful.log' 2>&1"
check "the binary is still there to run it with" test -x "$stateful/bin/sloop"
check "it names the command that does know what to remove" \
    grep -Fq 'uninstall' "$work/stateful.log"
check "and the flag that says take only the binary" \
    grep -Fq -- '--binary-only' "$work/stateful.log"
check "--binary-only then does exactly that" \
    sh -c "HOME='$stateful' sh '$root/install/uninstall.sh' --dir '$stateful/bin' --binary-only"
check "the state it was told to leave alone is still there" test -d "$stateful/.config/sloop"
check_not "and the binary is gone" test -e "$stateful/bin/sloop"

# ----------------------------------------------------------------------------------- end

printf '\ninstall-roundtrip: %s of %s checks passed.\n' "$passed" "$checks"
exit "$failed"
