#!/usr/bin/env bash
# Local VAAPI frontend to FilmCraft's official Linux packager. No dependency bundler.
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'Requires native Linux x86_64.' >&2; exit 1; }
BINARY=''
case ${1:-} in
    --binary) [[ $# == 2 ]] || exit 2; BINARY=$(realpath -e -- "$2") ;;
    --help|-h) echo 'Usage: scripts/build-appimage.sh [--binary /absolute/already-built/filmcraft]'; exit 0 ;;
    '') [[ $# == 0 ]] || exit 2 ;;
    *) echo 'Unknown argument; use --help.' >&2; exit 2 ;;
esac
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
VERSION=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')
[[ $VERSION =~ ^[0-9A-Za-z.+-]+$ ]] || { echo 'Invalid workspace version.' >&2; exit 1; }
# Use Cargo artifact paths, including custom target directories.
WORK=$(mktemp -d "${TMPDIR:-/tmp}/filmcraft-appimage.XXXXXXXX")
trap 'rm -rf -- "$WORK"' EXIT
PACKAGES=(-p filmcraft-cli)
[[ -n $BINARY ]] || PACKAGES+=(-p filmcraft)
cargo build --release --locked "${PACKAGES[@]}" --message-format=json-render-diagnostics > "$WORK/build.jsonl"
python3 - "$WORK/build.jsonl" "$WORK" <<'PY'
import json,pathlib,sys
for line in open(sys.argv[1]):
    m=json.loads(line)
    name=m.get('target',{}).get('name')
    if m.get('reason')=='compiler-artifact' and m.get('executable') and name in ('filmcraft','filmcraft-cli'):
        pathlib.Path(sys.argv[2],name).write_text(m['executable'])
PY
[[ -n $BINARY ]] || BINARY=$(cat "$WORK/filmcraft")
CLI=$(cat "$WORK/filmcraft-cli")
mkdir "$WORK/bin"
for name in filmcraft filmcraft-cli; do
    bin=$BINARY
    [[ $name == filmcraft ]] || bin=$CLI
    [[ -x $bin ]] || { echo "Missing executable: $bin" >&2; exit 1; }
    python3 - "$bin" <<'PY'
import sys
with open(sys.argv[1],'rb') as f: h=f.read(20)
if h[:6]!=b'\x7fELF\x02\x01' or int.from_bytes(h[18:20],'little')!=62:
    sys.exit('Not a native x86_64 ELF binary: '+sys.argv[1])
PY
    [[ $("$bin" --version) == "$name $VERSION" ]] || { echo "Binary version does not match workspace: $bin" >&2; exit 1; }
    install -m755 "$bin" "$WORK/bin/$name"
done
# An isolated workspace lets the official packager create a fresh AppDir each run.
# Its AppRun is a direct symlink, so it cannot override the host GPU loader paths.
DIST="$WORK/output" FILMCRAFT_VERSION="$VERSION" \
    FILMCRAFT_PACKAGE_BIN_DIR="$WORK/bin" FILMCRAFT_PACKAGE_WORK="$WORK/package" \
    "$ROOT/packaging/linux/package.sh" --skip-build --formats appimage
APPDIR="$WORK/package/FilmCraft.AppDir"
# Audit the complete payload: no shared libraries or graphics-driver manifests permitted.
python3 - "$APPDIR" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[1])
for f in p.rglob('*'):
    if '.so' in f.name or f.name in ('icd.d','dri','vulkan','glvnd'):
        sys.exit('Unexpected host/driver library in AppDir: '+str(f))
assert (p/'AppRun').is_symlink() and (p/'AppRun').readlink()==pathlib.Path('usr/bin/filmcraft')
assert (p/'usr/share/doc/filmcraft/LICENSE-MIT').is_file()
print('AppDir library/driver audit: PASS (no shared libraries bundled)')
PY
IMAGE="$WORK/output/filmcraft-$VERSION-linux-x86_64.AppImage"
chmod +x "$IMAGE"
# Like official CI: exercise the actual AppImage entry point without needing a display/FUSE.
APPIMAGE_EXTRACT_AND_RUN=1 "$IMAGE" --version
mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/FilmCraft-$VERSION-VAAPI-linux-x86_64.AppImage"
# Only replace the previous package after packaging and launch checks succeed.
mv -- "$IMAGE" "$OUT"
(cd "$ROOT/dist"; sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
printf '\nAppImage: %s\nFile size: %s bytes\nVersion: %s\nSHA256: %s\nLaunch test (--version): PASS\nGUI launch / VAAPI detection: NOT RUN (use scripts/test-appimage-vaapi.py)\n' \
    "$OUT" "$(stat -c %s "$OUT")" "$VERSION" "$(sha256sum "$OUT" | cut -d' ' -f1)"
