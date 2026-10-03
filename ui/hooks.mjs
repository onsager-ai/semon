import { execFileSync } from 'node:child_process';
const owned = '.githooks';
const git = (...args) => execFileSync('git', args, { encoding: 'utf8' }).trim();
let current = '';
try {
  current = git('config', '--get', 'core.hooksPath');
} catch (error) {
  if (error.status !== 1) throw error;
}
if (process.argv[2] === 'install') {
  if (current && current !== owned)
    throw new Error(
      `Existing core.hooksPath=${current}; preserve it and invoke ui/format.mjs --staged from your hook.`,
    );
  const { existsSync } = await import('node:fs');
  if (!current && existsSync(git('rev-parse', '--git-path', 'hooks/pre-commit')))
    throw new Error(
      'Existing pre-commit hook; preserve it and invoke ui/format.mjs --staged from your hook.',
    );
  git('config', '--local', 'core.hooksPath', owned);
} else if (process.argv[2] === 'remove') {
  const local = (() => {
    try {
      return git('config', '--local', '--get', 'core.hooksPath');
    } catch {
      return '';
    }
  })();
  if (local === owned) git('config', '--local', '--unset', 'core.hooksPath');
} else throw new Error('Use install or remove');
