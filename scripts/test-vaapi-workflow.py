#!/usr/bin/env python3
"""Real Git integration tests; Cargo is replaced ONLY inside temporary fixtures."""
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SOURCE = pathlib.Path(__file__).resolve().parents[1]


class Workflow(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='filmcraft workflow ')
        self.addCleanup(self.tmp.cleanup)
        self.home = pathlib.Path(self.tmp.name)
        self.repo = self.home / 'checkout'
        self.repo.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_SYSTEM='/dev/null',
                        GIT_AUTHOR_NAME='Workflow Test', GIT_AUTHOR_EMAIL='test@example.invalid',
                        GIT_COMMITTER_NAME='Workflow Test', GIT_COMMITTER_EMAIL='test@example.invalid',
                        GIT_EDITOR='true', GIT_SEQUENCE_EDITOR='true')
        self.run_cmd(['git', 'init', '--bare', str(self.home / 'origin.git')])
        self.git('init', '-b', 'main')
        for name, value in {
            'Cargo.toml': '[workspace]\nmembers = ["apps/filmcraft"]\n',
            'apps/filmcraft/Cargo.toml': '[package]\nname = "filmcraft"\nversion = "0.1.0"\n',
            'crates/platform/Cargo.toml': '[package]\nname = "filmcraft-platform"\n',
            'feature.txt': 'base\n', 'upstream.txt': 'base\n',
        }.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value)
        self.git('add', '.')
        self.git('commit', '-m', 'Upstream base')
        self.base = self.git('rev-parse', 'HEAD')
        self.git('remote', 'add', 'origin', str(self.home / 'origin.git'))
        self.git('push', 'origin', 'main')
        self.run_cmd(['git', 'clone', '-b', 'main', str(self.home / 'origin.git'), str(self.home / 'upstream')])
        self.git('switch', '-c', 'vaapi-hardware-encode')
        (self.repo / 'scripts').mkdir()
        for name in ['update-filmcraft-vaapi.sh', 'scripts/vaapi-common.sh',
                     'scripts/export-vaapi-patch.sh', 'scripts/apply-vaapi-patch.sh']:
            shutil.copy2(SOURCE / name, self.repo / name)
        packager = self.repo / 'scripts/build-appimage.sh'
        packager.write_text('#!/usr/bin/env bash\necho "appimage $*" >> "$CARGO_TEST_LOG"\n[[ ${FAIL_CARGO:-} != appimage ]]\n')
        packager.chmod(0o755)
        (self.repo / 'feature.txt').write_text('local VAAPI\n')
        self.git('add', '.')
        self.git('commit', '-m', 'VAAPI implementation and workflow fixture')
        self.tip = self.git('rev-parse', 'HEAD')
        bindir = self.home / 'bin'
        bindir.mkdir()
        cargo = bindir / 'cargo'
        cargo.write_text('''#!/usr/bin/env python3
import json,os,pathlib,sys
args=sys.argv[1:]
with open(os.environ['CARGO_TEST_LOG'],'a') as f: f.write(' '.join(args)+'\\n')
if os.environ.get('FAIL_CARGO') == args[0]: sys.exit(17)
if args[0]=='metadata':
 print(json.dumps({'packages':[{'name':'filmcraft','manifest_path':str(pathlib.Path('apps/filmcraft/Cargo.toml').resolve()),'default_run':'filmcraft','targets':[{'name':'filmcraft','kind':['bin']}]}]}))
if args[0]=='build':
 print(json.dumps({'reason':'compiler-artifact','target':{'name':'filmcraft'},'executable':str(pathlib.Path('target/release/filmcraft').resolve())}))
''')
        cargo.chmod(0o755)
        self.env['PATH'] = str(bindir) + os.pathsep + self.env['PATH']
        self.env['CARGO_TEST_LOG'] = str(self.home / 'cargo.log')

    def run_cmd(self, args, cwd=None, ok=True):
        p = subprocess.run(args, cwd=cwd or self.repo, env=self.env, text=True,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if ok:
            self.assertEqual(p.returncode, 0, p.stdout)
        else:
            self.assertNotEqual(p.returncode, 0, p.stdout)
        return p.stdout.strip()

    def git(self, *args):
        return self.run_cmd(['git', *args])

    def update(self, *args, ok=True):
        return self.run_cmd([str(SOURCE / 'update-filmcraft-vaapi.sh'), *args], ok=ok)

    def advance(self, conflict=False):
        upstream = self.home / 'upstream'
        (upstream / ('feature.txt' if conflict else 'upstream.txt')).write_text('new upstream\n')
        for args in [('add', '.'), ('commit', '-m', 'New upstream'), ('push', 'origin', 'main')]:
            self.run_cmd(['git', *args], cwd=upstream)
        return self.run_cmd(['git', 'rev-parse', 'HEAD'], cwd=upstream)

    def backup(self):
        refs = self.git('for-each-ref', '--format=%(refname:short)', 'refs/heads/backup-vaapi-').splitlines()
        # for-each-ref prefix matching requires glob on some Git versions.
        if not refs:
            refs = self.git('branch', '--list', 'backup-vaapi-*', '--format=%(refname:short)').splitlines()
        self.assertEqual(len(refs), 1)
        return refs[0]

    def test_success_and_backup_with_update_refs_enabled(self):
        upstream = self.advance()
        self.git('config', 'rebase.updateRefs', 'true')
        result = self.update()
        self.assertIn('FilmCraft upstream update: SUCCESS', result)
        self.assertEqual(self.git('rev-parse', 'main'), upstream)
        self.assertEqual(self.git('rev-parse', self.backup()), self.tip)
        self.assertEqual(self.git('branch', '--show-current'), 'vaapi-hardware-encode')
        self.assertEqual((self.repo / 'feature.txt').read_text(), 'local VAAPI\n')
        self.assertEqual(self.git('status', '--porcelain'), '')
        self.assertEqual(self.git('config', 'rerere.enabled'), 'true')
        commands = (self.home / 'cargo.log').read_text()
        for cmd in ['fmt --check', 'check --workspace', 'clippy --workspace', 'test --workspace',
                    'xtask layers', 'xtask assets', 'xtask wasm', 'build --release --locked -p filmcraft --bin filmcraft', 'appimage --binary']:
            self.assertIn(cmd, commands)

    def test_dirty_and_dry_run(self):
        before = self.git('show-ref')
        self.assertIn('Preflight PASS', self.update('--dry-run'))
        self.assertEqual(self.git('show-ref'), before)
        self.assertFalse((self.home / 'cargo.log').exists())
        (self.repo / 'untracked.txt').write_text('keep me')
        self.assertIn('not clean', self.update(ok=False))
        self.assertEqual(self.git('show-ref'), before)
        self.assertEqual((self.repo / 'untracked.txt').read_text(), 'keep me')
        (self.repo / 'untracked.txt').unlink()
        (self.repo / 'feature.txt').write_text('unstaged edit')
        self.assertIn('not clean', self.update(ok=False))
        self.git('add', 'feature.txt')
        self.assertIn('not clean', self.update(ok=False))

    def test_conflict_abort_retry_and_resume(self):
        upstream = self.advance(conflict=True)
        output = self.update(ok=False)
        self.assertIn('feature.txt', output)
        self.assertIn('git rebase --abort', output)
        self.assertFalse((self.home / 'cargo.log').exists())
        self.assertEqual(self.git('rev-parse', self.backup()), self.tip)
        self.assertIn('already active', self.update(ok=False))
        self.git('rebase', '--abort')
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.tip)
        self.assertEqual(self.git('rev-parse', 'main'), upstream)
        self.update(ok=False)  # Retry is supported even though main advanced before abort.
        (self.repo / 'feature.txt').write_text('new upstream plus VAAPI\n')
        self.git('add', 'feature.txt')
        self.git('rebase', '--continue')
        self.assertIn('release build: PASS', self.update('--verify-only'))

    def test_diverged_main(self):
        self.git('switch', 'main')
        (self.repo / 'private.txt').write_text('local main commit')
        self.git('add', '.')
        self.git('commit', '-m', 'Local main divergence')
        localmain = self.git('rev-parse', 'HEAD')
        self.git('switch', 'vaapi-hardware-encode')
        self.advance()
        self.assertIn('diverged', self.update(ok=False))
        self.assertEqual(self.git('rev-parse', 'main'), localmain)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.tip)

    def test_validation_failure_stops_build(self):
        self.advance()
        self.env['FAIL_CARGO'] = 'test'
        self.assertIn('FAILED (cargo test)', self.update(ok=False))
        self.assertNotIn('build --release', (self.home / 'cargo.log').read_text())
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_packaging_failure_does_not_report_update_success(self):
        self.advance()
        self.env['FAIL_CARGO'] = 'appimage'
        result = self.update(ok=False)
        self.assertIn('FAILED (AppImage packaging)', result)
        self.assertNotIn('FilmCraft upstream update: SUCCESS', result)
        self.assertEqual(self.git('status', '--porcelain'), '')

    def export(self):
        output = self.home / 'portable patches'
        self.run_cmd([str(SOURCE / 'scripts/export-vaapi-patch.sh'), str(output)])
        return output

    def test_export_apply_and_checksum_rejection(self):
        patches = self.export()
        original = self.git('rev-parse', 'HEAD^{tree}')
        self.git('switch', 'main')
        (patches / 'tip-commit').write_text('damaged')
        self.assertIn('Invalid or damaged', self.run_cmd([str(SOURCE / 'scripts/apply-vaapi-patch.sh'), str(patches), 'portable'], ok=False))
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.base)
        (patches / 'tip-commit').write_text(self.tip + '\n')
        self.run_cmd([str(SOURCE / 'scripts/apply-vaapi-patch.sh'), str(patches), 'portable'])
        self.assertEqual(self.git('rev-parse', 'HEAD^{tree}'), original)
        self.assertEqual(self.git('rev-parse', 'main'), self.base)

    def test_apply_conflict_and_abort(self):
        patches = self.export()
        upstream = self.advance(conflict=True)
        self.git('fetch', 'origin')
        self.git('switch', 'main')
        self.git('merge', '--ff-only', 'origin/main')
        out = self.run_cmd([str(SOURCE / 'scripts/apply-vaapi-patch.sh'), str(patches), 'portable'], ok=False)
        self.assertIn('git am --abort', out)
        self.git('am', '--abort')
        self.assertEqual(self.git('rev-parse', 'HEAD'), upstream)
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_missing_remote_and_fetch_failure(self):
        self.git('remote', 'remove', 'origin')
        self.assertIn('origin is missing', self.update(ok=False))
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.tip)
        self.git('remote', 'add', 'origin', str(self.home / 'missing.git'))
        self.update(ok=False)
        self.assertEqual(self.git('rev-parse', self.backup()), self.tip)
        self.assertEqual(self.git('branch', '--show-current'), 'vaapi-hardware-encode')

    def test_multi_commit_export_on_newer_upstream(self):
        (self.repo / 'second.txt').write_text('another VAAPI commit')
        self.git('add', '.')
        self.git('commit', '-m', 'Second custom change')
        patches = self.export()
        self.assertEqual(len((patches / 'series').read_text().splitlines()), 2)
        upstream = self.advance()
        self.git('fetch', 'origin')
        self.git('switch', 'main')
        self.git('merge', '--ff-only', 'origin/main')
        self.run_cmd([str(SOURCE / 'scripts/apply-vaapi-patch.sh'), str(patches), 'portable'])
        self.assertEqual(self.git('rev-parse', 'main'), upstream)
        self.assertEqual((self.repo / 'feature.txt').read_text(), 'local VAAPI\n')
        self.assertEqual((self.repo / 'upstream.txt').read_text(), 'new upstream\n')
        self.assertEqual(self.git('rev-list', '--count', 'main..HEAD'), '2')

    def test_detached_and_wrong_repository(self):
        self.git('switch', '--detach')
        self.assertIn('Detached HEAD', self.update(ok=False))
        self.assertIn('FilmCraft repository', self.run_cmd([str(SOURCE / 'update-filmcraft-vaapi.sh'), '--dry-run'], cwd=self.home/'origin.git', ok=False))


if __name__ == '__main__':
    unittest.main(verbosity=2)
