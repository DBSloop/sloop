#!/bin/sh
#
# Fail the build if the repository stops being one a stranger can trust.
#
# GitHub's community-standards checklist is a page somebody looks at once, ticks off, and
# never opens again -- and a file deleted in a tidy-up six months later takes an item off
# that page silently. So the checklist is a build step. Every file it names is checked for
# existence and for having content, the two links a reader is most likely to follow are
# checked for pointing at something real, and the README's install command is checked
# against the version the manifest actually publishes.
#
# **The README is one file, and this is the half that proves it.** `cli/Cargo.toml` sets
# `readme = "../README.md"`, so Cargo copies the repository's README into the crate when it
# packages it -- which is what makes the crates.io page and the GitHub page the same text
# with nothing to keep in step. That resolution is Cargo's, not ours, and a Cargo release
# could change it; `cargo package --list` is asked directly rather than assumed. The
# byte-for-byte comparison of the packaged copy runs in the same CI job, where a full
# `cargo package` is affordable.
#
# Run from the repository root.

set -eu

fail=0

problem() {
    # GitHub renders this as an annotation on the run; a plain shell sees an ordinary line.
    echo "::error::$1"
    printf '  %s\n' "$1" >&2
    fail=1
}

# --------------------------------------------------------------------------------- files

# Every item on the checklist, and the smallest size that means somebody wrote it rather
# than leaving a heading. The numbers are floors, not targets.
check_file() {
    path="$1"
    least="$2"

    if [ ! -f "$path" ]; then
        problem "$path is missing -- GitHub's community profile counts it"
        return
    fi

    size=$(wc -c < "$path" | tr -d ' ')
    if [ "$size" -lt "$least" ]; then
        problem "$path is $size bytes, which is too short to be the real thing"
        return
    fi

    echo "  ok  $path ($size bytes)"
}

check_file README.md 4000
check_file CONTRIBUTING.md 2000
check_file SECURITY.md 2000
check_file CODE_OF_CONDUCT.md 3000
check_file LICENSE-MIT 800
check_file LICENSE-APACHE 8000
check_file .github/pull_request_template.md 500
check_file .github/ISSUE_TEMPLATE/bug_report.yml 800
check_file .github/ISSUE_TEMPLATE/feature_request.yml 800
check_file .github/ISSUE_TEMPLATE/config.yml 100

# ------------------------------------------------------------------------- what they say

# A README that has lost the guarantee is a README that is selling something else. It is
# the first claim on the page, the first line of `--help`, and the reason the dependency
# check above it exists.
if [ -f README.md ] && ! grep -q "never leave your machine" README.md; then
    problem "README.md no longer makes the guarantee -- it is the claim the project is built on"
fi

# The three files a reader is pointed at from the README. A relative link that 404s on
# GitHub is the cheapest possible way to look unmaintained.
if [ -f README.md ]; then
    for linked in SECURITY.md CONTRIBUTING.md LICENSE-MIT LICENSE-APACHE; do
        if grep -q "]($linked)" README.md && [ ! -f "$linked" ]; then
            problem "README.md links to $linked, which is not there"
        fi
    done
fi

# `blank_issues_enabled: false` sends everybody through the templates, so a template that
# has stopped being offered leaves them with nowhere to go but the contact links.
if [ -f .github/ISSUE_TEMPLATE/config.yml ] &&
    grep -q "blank_issues_enabled: *false" .github/ISSUE_TEMPLATE/config.yml; then
    for form in bug_report feature_request; do
        [ -f ".github/ISSUE_TEMPLATE/$form.yml" ] ||
            problem "blank issues are off and $form.yml is missing -- nobody could file anything"
    done
fi

# ------------------------------------------------------------------------- the crate page

# `cargo install dbsloop` with no version cannot resolve a pre-release, so the README names
# one -- and a named version goes stale the moment the manifest moves. Checked rather than
# remembered, and the check disappears on its own when 1.0 removes the sentence.
if [ -f cli/Cargo.toml ] && [ -f README.md ]; then
    version=$(sed -n 's/^version = "\(.*\)"$/\1/p' cli/Cargo.toml | head -n 1)
    if [ -z "$version" ]; then
        problem "cli/Cargo.toml has no version, so nothing can be checked against it"
    elif grep -q 'cargo install dbsloop --version' README.md &&
        ! grep -q "cargo install dbsloop --version $version" README.md; then
        named=$(grep -o 'cargo install dbsloop --version [^ ]*' README.md | head -n 1)
        problem "README.md says '$named' and cli/Cargo.toml is at $version"
    fi
fi

# The crate's `readme` has to resolve to the repository's, or crates.io renders something
# else -- or, if Cargo ever stops accepting a path outside the package, nothing at all.
if [ -f cli/Cargo.toml ]; then
    if ! grep -q '^readme = "\.\./README\.md"$' cli/Cargo.toml; then
        problem "cli/Cargo.toml no longer points readme at the repository's README.md"
    elif command -v cargo > /dev/null 2>&1; then
        listed=$(cd cli && cargo package --list --allow-dirty 2> /dev/null | grep -c '^README\.md$' || true)
        if [ "${listed:-0}" -eq 0 ]; then
            problem "cargo would not put a README.md in the published crate"
        else
            echo "  ok  the published crate carries README.md"
        fi
    else
        echo "  -- cargo is not here, so the packaged README was not checked"
    fi
fi

# ---------------------------------------------------------------------------------- done

if [ "$fail" -ne 0 ]; then
    echo
    echo "The repository is short of what it tells strangers it is. See R23." >&2
    exit 1
fi

echo
echo "Every community-standards item is present."
