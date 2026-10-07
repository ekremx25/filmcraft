#!/usr/bin/env bash
set -Eeuo pipefail
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/vaapi-common.sh"
main() {
    if [[ ${1:-} == --help ]]; then echo 'Usage: ./scripts/export-vaapi-patch.sh [NEW-OUTPUT-DIRECTORY]'; return; fi
    [[ $# -le 1 ]] || fail 'Too many arguments.'
    # Resolve relative output paths from the caller directory before preflight changes cwd.
    local out
    out=$(realpath -m -- "${1:-patches/vaapi-$(date -u +%Y%m%d-%H%M%S)-$$}")
    preflight
    [[ $branch != main && $branch != backup-vaapi-* ]] || fail 'Export from the dedicated VAAPI branch.'
    local base
    base=$(git merge-base main HEAD) || fail 'No shared upstream main history.'
    [[ -z $(git rev-list --merges "$base"..HEAD) ]] || fail 'Patch export requires a linear custom stack.'
    [[ $(git rev-list --count "$base"..HEAD) -gt 0 ]] || fail 'No custom commits to export.'
    [[ ! -e $out ]] || fail 'Output already exists. Choose a new directory; existing patches are never overwritten.'
    mkdir -p -- "$(dirname -- "$out")"
    mkdir -- "$out"
    git format-patch --binary --full-index --no-signature --no-cover-letter --no-numbered --no-thread --base="$base" --output-directory "$out" "$base"..HEAD
    printf '%s\n' "$base" > "$out/base-commit"
    git rev-parse HEAD > "$out/tip-commit"
    # format-patch's numeric prefixes give the application order.
    (cd "$out"; printf '%s\n' ./*.patch | sed 's|^./||' > series
        sha256sum base-commit tip-commit series ./*.patch > SHA256SUMS)
    printf '\nExported %s custom commits from %s to %s\nBase revision: %s\n' "$(git rev-list --count "$base"..HEAD)" "$branch" "$out" "$base"
}
main "$@"
