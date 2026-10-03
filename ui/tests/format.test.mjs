import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, copyFile, symlink, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

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
