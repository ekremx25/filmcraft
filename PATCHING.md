# Maintaining the Linux VAAPI build

The normal workflow is a **dedicated Git branch rebased onto upstream `main`**.
Run from a clean checkout of that branch:

```bash
cd ~/filmcraft
./update-filmcraft-vaapi.sh
```

`main` contains upstream FilmCraft only. `vaapi-hardware-encode` contains the
VAAPI implementation and its maintenance tooling. Other dedicated branch names
work too: the updater uses the currently checked-out branch and refuses `main`,
backup branches, detached HEADs, and a branch with no custom commits.
Do not put unrelated work on this branch: export includes **every commit unique
to this branch above its common ancestor with main**, not a heuristic selection
based on filenames or commit messages. Merge commits are rejected.

## First installation

For this existing installation, the working VAAPI code belongs on
`vaapi-hardware-encode`. `origin` is your fork,
`https://github.com/ekremx25/filmcraft.git`; `upstream` is the original project,
`https://github.com/storytold/filmcraft.git`. The updater displays and fetches
only the upstream URL. It never changes remote URLs or pushes to any remote.

For a new machine, clone your maintained repository and check out its VAAPI
branch, or clone upstream and apply an exported patch series as described below.
Configure your own Git identity locally if needed:

```bash
git config user.name 'Your Name'
git config user.email 'your-address@example.com'
```

Requirements: Bash, Git, Python 3.11+ (standard library only), GNU coreutils,
Rust/Cargo, rustfmt, clippy, and the `wasm32-unknown-unknown` Rust target. Install
FilmCraft's native dependencies from [contributing](docs/contributing.md) and
VAAPI dependencies/drivers from [Linux VAAPI export](docs/linux-vaapi-export.md).
The hardware feature requires Linux libva headers, libclang for bindgen, a
working VA driver, and access to the render node. See [testing](docs/testing.md)
for optional ffmpeg/ffprobe test oracles. Missing oracles/hardware may cause
some tests to skip; `FILMCRAFT_REQUIRE_VAAPI=1` requires the real VAAPI tests.

```bash
rustup component add rustfmt clippy
rustup target add wasm32-unknown-unknown
./update-filmcraft-vaapi.sh --dry-run
```

Dry-run only checks local preconditions. It does not fetch, create refs, set
configuration, change branches, or run Cargo. Remote compatibility is checked
during the real update. Untracked, staged, or unstaged files block all commands;
commit your intended changes first. Nothing is automatically stashed.

## Using your GitHub fork

Use this remote model:

| Remote | Fetch URL | Purpose |
| --- | --- | --- |
| `origin` | `https://github.com/ekremx25/filmcraft.git` | Your fork; publish custom commits here |
| `upstream` | `https://github.com/storytold/filmcraft.git` | Read upstream `main`; never push here |

For a fresh clone of your fork:

```bash
git clone https://github.com/ekremx25/filmcraft.git
cd filmcraft
git remote add upstream https://github.com/storytold/filmcraft.git
git config remote.upstream.pushurl DISABLED
git config remote.pushDefault origin
git config branch.vaapi-hardware-encode.pushRemote origin
git fetch upstream
git branch --set-upstream-to=upstream/main main
git switch vaapi-hardware-encode
```

For an existing checkout using the old names (`origin` was storytold and `fork`
was ekremx25), first inspect `git remote -v`, then migrate **once**:

```bash
git remote rename origin upstream
git remote rename fork origin
git config remote.upstream.pushurl DISABLED
git config remote.pushDefault origin
git config branch.vaapi-hardware-encode.pushRemote origin
```

Renaming preserves remote-tracking refs and updates branch tracking. Do not
repeat this migration on a checkout already using the correct names. The local
upstream push URL is deliberately unusable; its fetch URL remains valid.
The updater has no push command and no fallback to origin if upstream is missing.

Publish to your fork explicitly: `git push -u origin vaapi-hardware-encode`.
After a rebase, a previously published custom branch has rewritten history;
coordinate with other users, inspect the changes, and use
`git push --force-with-lease origin vaapi-hardware-encode` when appropriate.
The updater itself never force-pushes (or pushes at all).

## Updating FilmCraft and reapplying VAAPI changes

```bash
git switch vaapi-hardware-encode
./update-filmcraft-vaapi.sh
```

The updater:

1. Checks the FilmCraft workspace, clean worktree, linear custom stack, Git
   identity, and absence of an unfinished Git operation.
2. Creates `backup-vaapi-YYYYMMDD-HHMMSS-PID` at the original custom tip. The
   timestamp is UTC; the PID avoids collisions. The backup is kept on success
   and failure. No existing backup is moved or deleted.
3. Enables local `rerere.enabled=true` and `rerere.autoupdate=false` so Git
   remembers resolutions but lets you review them before staging.
4. Fetches `upstream/main`, requires local `main` to be its ancestor, switches to
   `main`, and fast-forwards it. A rewritten upstream or local divergence stops
   the update for manual review. A `main` branch checked out in another worktree
   can also prevent switching; no worktree is modified forcibly.
5. Switches back and rebases the custom commits onto `upstream/main`. Autostash, automatic
   updating of other refs, autosquash, fork-point selection, and merge recreation
   are disabled for this rebase. This also protects backup refs when your global
   Git configuration enables `rebase.updateRefs`.
6. Runs the checks below, then builds the desktop release. Failures stop the
   workflow and leave the branch and backup available for inspection.

Run only one updater/apply command in a checkout at a time. Do not edit files or
run another Git operation in that checkout while it runs. Upstream scripts and
Cargo build scripts are trusted code, just as during an ordinary manual build.

## Checks and rebuilding FilmCraft

After a successful rebase, commands run **sequentially**:

```bash
cargo fmt --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo test --workspace --release --locked
cargo xtask layers
cargo xtask assets
cargo xtask wasm
cargo build --release --locked -p filmcraft --bin filmcraft
```

These are the repository's `cargo xtask ci` gates, with an additional explicit
workspace check. Codec tests use release mode, matching the documented CI.
The script discovers the desktop package and binary from Cargo metadata at
`apps/filmcraft/Cargo.toml`; currently both are `filmcraft`. It reports Cargo's
actual executable artifact path (normally `target/release/filmcraft`), including
custom target directories. It does not install or launch the binary.

To avoid excessive RAM use, build jobs default to one and test threads to two.
You can override `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS`. Release-profile
clippy/tests disable LTO and use 16 codegen units to reduce validation memory.
The final release build uses the repository's normal release settings and your
Cargo environment. Only one Cargo invocation runs at a time.

To validate/rebuild without fetching or rebasing, including after resolving a
conflict or fixing a failing test:

```bash
./update-filmcraft-vaapi.sh --verify-only
```

Commit any fixes before running it: this path also requires a clean custom
branch. A failed check does not undo a successful rebase. The summary never
reports success until all gates and the release build pass.

## Handling conflicts

The updater stops with the backup name and conflicting files. Inspect each
conflict, including any resolution remembered by rerere:

```bash
git status
git diff
# Edit the conflicting files and check their behavior.
git add <resolved-files>
git rebase --continue
```

Repeat if another commit conflicts. A commit whose change is already upstream
may be dropped by Git; review the rebase output. Do not skip a commit simply to
silence an error. When the rebase finishes:

```bash
./update-filmcraft-vaapi.sh --verify-only
```

## Aborting an update and recovering

While a rebase is active:

```bash
git rebase --abort
```

This restores the original custom branch. `main` may already have fast-forwarded;
that is expected. The updater can be retried from this state. A fetch failure or
main divergence also leaves the backup intact. After an interrupted branch
switch, inspect `git status` and switch back to your VAAPI branch if appropriate.

After a completed rebase, `git rebase --abort` no longer applies. To inspect or
build the saved implementation without discarding the new branch:

```bash
git branch --list 'backup-vaapi-*'
git switch -c vaapi-recovery <backup-name>
```

Backups remain until **you** choose to delete them. The scripts contain no hard
reset, forced checkout, clean, or automatic stash. Keep an external backup too;
a local branch is not protection against disk loss.

## Exporting portable patch files

On the clean custom branch:

```bash
./scripts/export-vaapi-patch.sh
# Or specify a new directory (existing directories are never overwritten):
./scripts/export-vaapi-patch.sh /tmp/filmcraft-vaapi-patches
```

The default is a new `patches/vaapi-<UTC timestamp>-<PID>/` directory. `patches/`
is ignored by Git. Keep exports outside the repository when transferring them.
`git format-patch --binary --full-index --base=...` preserves actual commits,
authors, messages, and binary changes. The integrated VAAPI implementation is
one coherent commit (platform, codecs, export, UI, tests, docs, and supporting
parity fixes); it is not artificially divided into independently broken codec
commits. Maintenance tooling is a separate commit. Future custom commits become
additional numbered patches.

The directory also contains `series` (ordered filenames), `base-commit`,
`tip-commit`, and `SHA256SUMS`. Checksums detect corruption, not authenticity:
only apply a series from a trusted source. Export does not fetch or move refs.

## Applying to another clean checkout

Keep the maintenance scripts together (the apply script sources
`vaapi-common.sh`). Run the apply script **from the target checkout**, using its
absolute location in the maintained source checkout:

```bash
git clone https://github.com/ekremx25/filmcraft.git ~/filmcraft-new
cd ~/filmcraft-new
git remote add upstream https://github.com/storytold/filmcraft.git
git config remote.upstream.pushurl DISABLED
git config remote.pushDefault origin
git fetch upstream
git switch main
git merge --ff-only upstream/main
git branch --set-upstream-to=upstream/main main
/path/to/maintained-filmcraft/scripts/apply-vaapi-patch.sh /tmp/filmcraft-vaapi-patches
```

The target must be clean and on `main`. A new branch named
`vaapi-hardware-encode` is created; pass a second argument to choose another new
branch. Existing destination branches are refused. Patch checksums, safe
filenames, ordered-series completeness, and base history are validated before
creating refs. The target must contain the exported base or descend from it;
fetch additional history first if a shallow clone lacks the base. A newer
upstream can still have semantic incompatibilities, which only review and tests
can detect.

After making a backup and enabling rerere, the script runs `git am --3way` and
the same checks/release build. `main` stays upstream-only. On conflict:

```bash
git status
# Resolve and review the files.
git add <resolved-files>
git am --continue
# Or cancel:
git am --abort
```

After finishing all patches, run `./update-filmcraft-vaapi.sh --verify-only`.
After aborting, the new branch remains at the original upstream tip; switch to
`main` and choose a different destination branch for a later retry. For routine
updates of an existing VAAPI branch, always prefer the rebase updater.

## Testing the maintenance scripts

```bash
python3 scripts/test-vaapi-workflow.py
```

Tests create isolated temporary repositories with separate bare origin (fork)
and upstream remotes. They verify that updates follow upstream while the fork
remains unchanged, and that verification invokes the packager after release
building and rejects a missing AppImage in `dist/`. They
exercise real Git updates, backup preservation, dirty-tree rejection, dry-run,
main divergence, conflict/abort/retry/continue, validation failure, patch
export/application, damaged metadata, and detached/wrong-repository rejection.
Cargo is replaced only in those test fixtures to test orchestration without
rebuilding FilmCraft repeatedly. These tests do not replace the real Rust gates
or hardware export tests. They never fetch or rebase your working installation.

## Linux x86_64 AppImage

The updater now packages an AppImage **after** all gates and the desktop release
build pass. It calls `scripts/build-appimage.sh`, a frontend to the existing
`packaging/linux/package.sh --skip-build --formats appimage`. Package creation
failure fails the update; the rebased branch and backup are retained.
`verify_build` also checks that the versioned AppImage exists, is nonempty and
executable in `dist/`, and prints that path in its final summary.

```bash
./scripts/build-appimage.sh
# Or reuse the desktop executable that was just built (CLI is checked/built by Cargo):
./scripts/build-appimage.sh --binary "$PWD/target/release/filmcraft"
./dist/FilmCraft-0.2.1-VAAPI-linux-x86_64.AppImage
```

The version comes from `[workspace.package].version`, not the example above.
Output: `dist/FilmCraft-<version>-VAAPI-linux-x86_64.AppImage`, with a `.sha256`
sidecar. The official packager supplies `ai.storyteller.filmcraft`, the desktop
entry, categories, AppStream and MIME metadata, and `assets/app-icon/hicolor`
icons. AppRun directly links to `usr/bin/filmcraft`; application arguments pass
through unchanged. Both FilmCraft and its CLI are included. Licence/attribution
notices are retained in the AppDir. Packaging uses a fresh temporary staging
area and replaces the old output only after its `--version` launch test passes.

No shared libraries are bundled. The complete AppDir is audited to reject them
and graphics-driver directories. In particular there is no Mesa, libva,
`*_drv_video.so`, Vulkan ICD, libdrm, libGL/EGL, or GPU kernel library in the
payload. No loader/driver environment variables are forced. `/dev/dri` is used
with the invoking user's normal permissions; AppImage is not a sandbox and the
launcher does not add device isolation or permission changes.

This is an install-free package, **not an all-distributions runtime**. Host
ALSA, glibc/libgcc, the applicable X11/Wayland libraries, Vulkan/OpenGL loader,
libva and the GPU driver must be installed. The build host's ABI matters: a
binary built on a recent distribution can fail on an older glibc. For broad
compatibility, build on the project's Ubuntu 22.04 release baseline instead of
copying glibc/Mesa out of a newer workstation. Test each target distribution.
Optional craft-fonts remain a build input exactly as in the official release.

The official packager uses `APPIMAGETOOL` if provided, then PATH, then downloads
AppImage's official continuous tool to the Cargo target directory. Building
therefore needs network access on first use; the tool may also fetch its runtime.
For controlled/offline builds, supply a preprovisioned tool/runtime as supported
by appimagetool. Packaging-tool versions are independent of FilmCraft's version.
Compression runs with the tool's default settings; Rust jobs default to one.
If FUSE is unavailable, use the official runtime's extraction mode:

```bash
APPIMAGE_EXTRACT_AND_RUN=1 ./dist/FilmCraft-0.2.1-VAAPI-linux-x86_64.AppImage
```

### Testing real VAAPI from the package

A `--version` test does not establish that a window opens or that VAAPI works.
Those statuses are explicitly reported as not run by the normal packaging step.
On an AMD machine with an active desktop, install `amdgpu_top` and the optional
ffmpeg/ffprobe **test oracles**, then run:

```bash
python3 scripts/test-appimage-vaapi.py dist/FilmCraft-0.2.1-VAAPI-linux-x86_64.AppImage
```

The test launches the actual AppImage using a separate profile and loopback
control port, renders 15 seconds of the procedural demo at 1920×1080/30fps in
Hardware mode, and requires a VAAPI encoder log plus open render-node access.
It captures the process's loaded host graphics libraries and its own PID's
amdgpu_top VCN usage, verifies all 450 H.264 frames using ffprobe/ffmpeg, takes a
screenshot, and closes only its own test application. On unified AMD video
engines the metric is `VCN_Unified`; with procedural source frames and no video
decoder in this test, the observed VCN work is hardware encoding.

Evidence, logs, the test export, screenshot, and `report.json` (path, size,
version, SHA256, launch/VAAPI/VCN results) are retained under
`target/appimage-vaapi-test-<timestamp>/`. Hardware testing is deliberately
explicit so routine updates also work without a logged-in graphical session or
on non-AMD machines. A failure exits nonzero and keeps the evidence for diagnosis.

The initial local VAAPI AppImage was tested on CachyOS with an RX 9070 XT:
normal FUSE GUI launch, host libva/Mesa loading and renderD128 access passed;
450 H.264 frames decoded successfully and the application's VCN Unified usage
peaked at 27%. ELF symbol inspection requires **glibc 2.44** for this workstation
build. These results describe this machine/package, not untested older distros.
