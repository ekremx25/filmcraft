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
`vaapi-hardware-encode`; keep `origin` pointing to
`https://github.com/storytold/filmcraft.git`. The scripts display the origin URL
before using it. They never change the remote URL.

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

This workflow reserves `origin` for upstream updates. Add your personal fork as
`fork` and publish the custom branch there; use your own GitHub URL:

```bash
git remote add fork https://github.com/YOUR-ACCOUNT/filmcraft.git
git push -u fork vaapi-hardware-encode
```

If you cloned your fork first, rename that remote to `fork` and add the original
repository as `origin` before running the updater:

```bash
git remote rename origin fork
git remote add origin https://github.com/storytold/filmcraft.git
```

Inspect `git remote -v` before making these changes. Do not rename an already
correct upstream remote. The updater fetches upstream but **never pushes**.
After a rebase, updating a previously published custom branch rewrites its
history. Coordinate with anyone using that branch, inspect the result, and use
an explicit `git push --force-with-lease fork vaapi-hardware-encode` yourself
when appropriate. The script never force-pushes on your behalf.

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
4. Fetches `origin/main`, requires local `main` to be its ancestor, switches to
   `main`, and fast-forwards it. A rewritten upstream or local divergence stops
   the update for manual review. A `main` branch checked out in another worktree
   can also prevent switching; no worktree is modified forcibly.
5. Switches back and rebases the custom commits onto `main`. Autostash, automatic
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
git clone https://github.com/storytold/filmcraft.git ~/filmcraft-new
cd ~/filmcraft-new
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

Tests create isolated temporary repositories and a local bare upstream. They
exercise real Git updates, backup preservation, dirty-tree rejection, dry-run,
main divergence, conflict/abort/retry/continue, validation failure, patch
export/application, damaged metadata, and detached/wrong-repository rejection.
Cargo is replaced only in those test fixtures to test orchestration without
rebuilding FilmCraft repeatedly. These tests do not replace the real Rust gates
or hardware export tests. They never fetch or rebase your working installation.
