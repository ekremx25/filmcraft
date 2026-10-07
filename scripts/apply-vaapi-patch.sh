#!/usr/bin/env bash
set -Eeuo pipefail
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/vaapi-common.sh"
main() {
    if [[ ${1:-} == --help ]]; then echo 'Usage: /path/to/scripts/apply-vaapi-patch.sh PATCH-DIRECTORY [NEW-BRANCH]'; return; fi
    [[ $# -ge 1 && $# -le 2 ]] || fail 'Provide a patch directory and optional new branch name.'
    local patches new_branch base
    patches=$(realpath -e -- "$1")
    new_branch=${2:-vaapi-hardware-encode}
    preflight
    [[ $branch == main ]] || fail 'Start from a clean upstream main checkout.'
    git check-ref-format --branch "$new_branch" >/dev/null
    [[ $new_branch != main && $new_branch != backup-vaapi-* ]] || fail 'Choose a dedicated custom branch name.'
    if git show-ref --verify --quiet "refs/heads/$new_branch"; then fail 'Destination branch already exists; use its rebase workflow instead.'; fi
    identity
    # Treat metadata as data, never shell code. Verify the complete ordered series.
    base=$(python3 - "$patches" <<'PY'
import hashlib,pathlib,re,sys
p=pathlib.Path(sys.argv[1]); names=(p/'series').read_text().splitlines()
assert names and len(names)==len(set(names)), 'Empty or duplicate series'
assert all(re.fullmatch(r'[0-9]{4,}-[A-Za-z0-9_.-]+\.patch',n) for n in names), 'Unsafe patch filename'
expected=set(names)|{'base-commit','tip-commit','series'}
seen=set()
for line in (p/'SHA256SUMS').read_text().splitlines():
 digest, name=line.split('  ',1); name=name.removeprefix('./')
 assert name in expected and name not in seen, 'Unexpected/duplicate checksum entry'
 f=p/name
 assert not f.is_symlink() and f.is_file(), 'Missing file or symlink'
 assert hashlib.sha256(f.read_bytes()).hexdigest()==digest, 'Checksum mismatch: '+name
 seen.add(name)
assert seen==expected, 'Incomplete checksums'
base=(p/'base-commit').read_text().strip()
assert re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}',base), 'Invalid base commit'
print(base)
PY
    ) || fail 'Invalid or damaged patch series; no refs changed.'
    git cat-file -e "$base^{commit}" || fail 'Patch base is unavailable. Fetch its upstream history first.'
    git merge-base --is-ancestor "$base" HEAD || fail 'Checkout must be the exported upstream base or its descendant. Nothing applied.'
    backup
    git config --local rerere.enabled true
    git config --local rerere.autoupdate false
    git switch -c "$new_branch"
    local names=() name
    while IFS= read -r name; do names+=("$patches/$name"); done < "$patches/series"
    if ! git -c rerere.autoupdate=false am --3way -- "${names[@]}"; then
        conflicts am
        echo 'After completing git am, run ./update-filmcraft-vaapi.sh --verify-only (from the applied series).'
        exit 1
    fi
    echo 'Patch series applied. Checking compatibility with the current upstream through the full build/test gates.'
    verify_build
}
main "$@"
