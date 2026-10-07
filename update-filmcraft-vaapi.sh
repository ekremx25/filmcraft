#!/usr/bin/env bash
# All functions are loaded before checkout/rebase can replace these files.
set -Eeuo pipefail
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/scripts/vaapi-common.sh"
main() {
    case ${1:-} in
        --help|-h) echo 'Usage: ./update-filmcraft-vaapi.sh [--dry-run | --verify-only]'; return ;;
        ''|--dry-run|--verify-only) ;;
        *) fail 'Unknown argument. Use --help.' ;;
    esac
    [[ $# -le 1 ]] || fail 'Too many arguments.'
    preflight
    [[ $branch != main && $branch != backup-vaapi-* ]] || fail 'Switch to the dedicated VAAPI branch first (normally vaapi-hardware-encode).'
    git show-ref --verify --quiet refs/heads/main || fail 'Local main is missing.'
    git merge-base main HEAD >/dev/null || fail 'main and the VAAPI branch have unrelated histories.'
    [[ -z $(git rev-list --merges main..HEAD) ]] || fail 'The custom stack contains merge commits. Review/linearize it manually first.'
    [[ $(git rev-list --count main..HEAD) -gt 0 ]] || fail 'No committed VAAPI changes above main. Commit the implementation first.'
    if [[ ${1:-} == --verify-only ]]; then verify_build; return; fi
    git remote get-url upstream >/dev/null || fail 'upstream is missing; add https://github.com/storytold/filmcraft.git as upstream.'
    identity
    printf 'VAAPI branch: %s\nUpstream remote: %s\n' "$branch" "$(git remote get-url upstream)"
    if [[ ${1:-} == --dry-run ]]; then
        echo 'Preflight PASS. No fetch, configuration, refs, checkout, or builds changed. Remote compatibility is checked during an actual update.'
        return
    fi
    backup
    git config --local rerere.enabled true
    # Do not automatically stage remembered conflict resolutions.
    git config --local rerere.autoupdate false
    git fetch --no-tags upstream +refs/heads/main:refs/remotes/upstream/main
    git merge-base --is-ancestor main upstream/main || fail "Local main diverged from upstream/main. No branch was reset. Backup: $backup_ref"
    # Explicit fast-forward only; main may be locked by another worktree, in which case stop.
    git -c submodule.recurse=false switch main
    if ! git -c merge.autoStash=false merge --ff-only upstream/main; then
        git -c submodule.recurse=false switch "$branch"
        fail "Could not fast-forward main. Backup: $backup_ref"
    fi
    git -c submodule.recurse=false switch "$branch"
    if ! git -c rebase.autoStash=false -c rebase.updateRefs=false -c rerere.autoupdate=false rebase --no-autosquash --no-fork-point --no-rebase-merges upstream/main; then
        conflicts rebase
        echo 'After completing the rebase, run ./update-filmcraft-vaapi.sh --verify-only'
        exit 1
    fi
    update_result=SUCCESS
    verify_build
}
main "$@"
