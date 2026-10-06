import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, copyFile, symlink, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('hook skips empty, Rust/docs-only and deleted UI changes without Node or Prettier', async () => {
  const root = await mkdtemp(join(tmpdir(), 'semon-hook-scope-'));
  const run = (cmd, args, options = {}) =>
    spawnSync(cmd, args, { cwd: root, encoding: 'utf8', ...options });
  const git = (...args) => {
    const result = run('git', args);
    assert.equal(result.status, 0, result.stderr);
  };
  try {
    await mkdir(join(root, '.githooks'));
    await mkdir(join(root, 'ui/src'), { recursive: true });
    await mkdir(join(root, 'bin'));
    await copyFile(
      new URL('../../.githooks/pre-commit', import.meta.url),
      join(root, '.githooks/pre-commit'),
    );
    const gitPath = spawnSync('sh', ['-c', 'command -v git'], { encoding: 'utf8' }).stdout.trim();
    await symlink(gitPath, join(root, 'bin/git'));
    const hook = () =>
      run('/bin/sh', ['.githooks/pre-commit'], {
        env: { ...process.env, PATH: join(root, 'bin') },
      });
    git('init');
    git('config', 'user.email', 'test@example.invalid');
    git('config', 'user.name', 'Test');
    await writeFile(join(root, 'ui/src/deleted.ts'), 'const value = 1;\n');
    git('add', '.');
    git('commit', '-m', 'fixture');
    assert.equal(hook().status, 0);
    await writeFile(join(root, 'README.md'), 'Documentation\n');
    await writeFile(join(root, 'lib.rs'), 'fn main() {}\n');
    git('add', 'README.md', 'lib.rs');
    assert.equal(hook().status, 0);
    git('rm', 'ui/src/deleted.ts');
    assert.equal(hook().status, 0);
    await mkdir(join(root, 'ui/src'), { recursive: true });
    await writeFile(join(root, 'ui/src/a space.tsx'), 'const value=2\n');
    git('add', 'ui/src/a space.tsx');
    const required = hook();
    assert.equal(required.status, 1);
    assert.match(required.stderr, /Install Node 22\+/);
    const missingDependencies = run('/bin/sh', ['.githooks/pre-commit']);
    assert.equal(missingDependencies.status, 1);
    assert.match(missingDependencies.stderr, /npm --prefix ui ci/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('staged checks preserve partial staging, spaces, renames/deletions and existing hooks', async () => {
  const root = await mkdtemp(join(tmpdir(), 'semon-format-'));
  const run = (cmd, args) => spawnSync(cmd, args, { cwd: root, encoding: 'utf8' });
  const git = (...args) => {
    const r = run('git', args);
    assert.equal(r.status, 0, r.stderr);
    return r.stdout;
  };
  try {
    await mkdir(join(root, 'ui/src'), { recursive: true });
    for (const path of ['ui/format.mjs', 'ui/hooks.mjs', '.prettierrc.json'])
      await copyFile(new URL('../../' + path, import.meta.url), join(root, path));
    await symlink(
      new URL('../node_modules', import.meta.url).pathname,
      join(root, 'ui/node_modules'),
    );
    git('init');
    git('config', 'user.email', 'test@example.invalid');
    git('config', 'user.name', 'Test');
    const file = join(root, 'ui/src/a space.ts');
    await writeFile(file, 'const a = 1;\n');
    git('add', '.');
    git('commit', '-m', 'fixture');
    await writeFile(file, 'const a=2\n');
    git('add', 'ui/src/a space.ts');
    await writeFile(file, 'const a = 3;\n');
    const beforeIndex = await readFile(join(root, '.git/index'));
    const beforeWork = await readFile(file);
    const bad = run(process.execPath, ['ui/format.mjs', '--staged']);
    assert.equal(bad.status, 1);
    assert.match(bad.stderr, /a space.ts/);
    assert.deepEqual(await readFile(join(root, '.git/index')), beforeIndex);
    assert.deepEqual(await readFile(file), beforeWork);
    await writeFile(file, 'const a = 2;\n');
    git('add', 'ui/src/a space.ts');
    await writeFile(file, 'const a=3\n');
    assert.equal(run(process.execPath, ['ui/format.mjs', '--staged']).status, 0);
    git('mv', 'ui/src/a space.ts', 'ui/src/renamed space.ts');
    assert.equal(run(process.execPath, ['ui/format.mjs', '--staged']).status, 0);
    git('rm', '-f', 'ui/src/renamed space.ts');
    assert.equal(run(process.execPath, ['ui/format.mjs', '--staged']).status, 0);
    git('config', '--local', 'core.hooksPath', 'user-hooks');
    assert.notEqual(run(process.execPath, ['ui/hooks.mjs', 'install']).status, 0);
    assert.equal(git('config', '--get', 'core.hooksPath').trim(), 'user-hooks');
    git('config', '--local', '--unset', 'core.hooksPath');
    await writeFile(join(root, '.git/hooks/pre-commit'), '#!/bin/sh\n');
    assert.notEqual(run(process.execPath, ['ui/hooks.mjs', 'install']).status, 0);
    await rm(join(root, '.git/hooks/pre-commit'));
    assert.equal(run(process.execPath, ['ui/hooks.mjs', 'install']).status, 0);
    assert.equal(git('config', '--local', '--get', 'core.hooksPath').trim(), '.githooks');
    assert.equal(run(process.execPath, ['ui/hooks.mjs', 'remove']).status, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
