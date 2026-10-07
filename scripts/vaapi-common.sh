#!/usr/bin/env bash
# Shared by the maintenance commands. Source before changing branches.
set -Eeuo pipefail
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
preflight() {
    command -v python3 >/dev/null || fail 'Python 3.11+ is required.'
    root=$(git rev-parse --show-toplevel 2>/dev/null) || fail 'Run inside the FilmCraft repository.'
    cd "$root"
    python3 - <<'PY' || fail 'This is not a FilmCraft workspace (Python 3.11+ required).'
import pathlib, tomllib
p = pathlib.Path('.')
w = tomllib.loads((p/'Cargo.toml').read_text())
a = tomllib.loads((p/'apps/filmcraft/Cargo.toml').read_text())
assert 'workspace' in w and a['package']['name'] == 'filmcraft'
assert (p/'crates/platform/Cargo.toml').is_file()
PY
    for state in rebase-merge rebase-apply MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD BISECT_LOG; do
        [[ ! -e $(git rev-parse --git-path "$state") ]] || fail "Git operation already active: $state. Finish or abort it first."
    done
    [[ -z $(git status --porcelain=v1 --untracked-files=all) ]] || fail 'Working tree is not clean. Commit or move your changes first; nothing was stashed or discarded.'
    branch=$(git symbolic-ref --quiet --short HEAD) || fail 'Detached HEAD: switch to your maintained branch first.'
}
backup() {
    backup_ref="backup-vaapi-$(date -u +%Y%m%d-%H%M%S)-$$"
    git branch "$backup_ref" HEAD
    printf 'Backup branch: %s (%s)\n' "$backup_ref" "$(git rev-parse HEAD)"
}
identity() { git var GIT_AUTHOR_IDENT >/dev/null; git var GIT_COMMITTER_IDENT >/dev/null; }
conflicts() {
    printf '\nConflicting files:\n'
    git diff --name-only --diff-filter=U
    printf '\nInspect: git status\nResolve files, then: git add <resolved-files>\nContinue: git %s --continue\nCancel: git %s --abort\nBackup: %s\n' "$1" "$1" "$backup_ref"
}
verify_build() {
    command -v cargo >/dev/null || fail 'cargo is required.'
    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
    local step='initialization' fmt='NOT RUN' check='NOT RUN' tests='NOT RUN' release='NOT RUN' binary='NOT BUILT'
    # This trap also reports interrupted builds; the rebased branch is never reset.
    trap 'printf "\nFilmCraft validation: FAILED (%s)\ncargo fmt: %s\ncargo check: %s\ncargo test: %s\nrelease build: %s\nResolve the failure, then run ./update-filmcraft-vaapi.sh --verify-only\n" "$step" "$fmt" "$check" "$tests" "$release" >&2' ERR
    step='cargo fmt'; cargo fmt --check; fmt=PASS
    step='cargo check'; cargo check --workspace --locked; check=PASS
    # Same release-profile gates as cargo xtask ci, with lower-memory validation.
    step='cargo clippy'
    CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo clippy --workspace --all-targets --release --locked -- -D warnings
    step='cargo test'
    CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo test --workspace --release --locked
    tests=PASS
    for gate in layers assets wasm; do step="cargo xtask $gate"; cargo xtask "$gate"; done
    step='package discovery'
    local metadata package executable buildlog
    metadata=$(cargo metadata --no-deps --format-version 1 --locked)
    read -r package executable < <(python3 -c '
import json,sys,pathlib
m=json.load(sys.stdin)
p=next(p for p in m["packages"] if pathlib.Path(p["manifest_path"]).resolve()==pathlib.Path("apps/filmcraft/Cargo.toml").resolve())
b=[t["name"] for t in p["targets"] if "bin" in t["kind"]]
n=p.get("default_run") or (b[0] if len(b)==1 else None)
assert n in b, "Cannot select the desktop binary"
print(p["name"],n)
' <<< "$metadata")
    [[ -n $package && -n $executable ]] || fail 'Desktop package/binary discovery failed.'
    step='release build'
    buildlog=$(mktemp)
    # Capture Cargo's actual artifact path, including custom target directories/targets.
    if cargo build --release --locked -p "$package" --bin "$executable" --message-format=json-render-diagnostics >"$buildlog"; then
        binary=$(python3 -c '
import json,sys
paths=[]
for line in open(sys.argv[1]):
 m=json.loads(line)
 if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["name"]==sys.argv[2]: paths.append(m["executable"])
assert paths, "No executable reported by Cargo"
print(paths[-1])
' "$buildlog" "$executable")
        rm "$buildlog"
    else
        printf 'Build output retained at %s\n' "$buildlog" >&2
        false
    fi
    release=PASS
    step='AppImage packaging'
    if [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]]; then
        "$root/scripts/build-appimage.sh" --binary "$binary"
    else
        echo 'AppImage packaging: SKIPPED (requires Linux x86_64)'
    fi
    trap - ERR
    printf '\nFilmCraft upstream update: %s\nUpstream revision: %s\nVAAPI patch: APPLIED\ncargo fmt: %s\ncargo check: %s\ncargo test: %s\nrelease build: %s\nbinary: %s\n' "${update_result:-NOT REQUESTED (validation only)}" "$(git rev-parse main 2>/dev/null || git rev-parse HEAD)" "$fmt" "$check" "$tests" "$release" "$binary"
}
