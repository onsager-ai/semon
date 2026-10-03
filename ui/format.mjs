import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import * as prettier from 'prettier';

const root = fileURLToPath(new URL('..', import.meta.url));
// Byte-contract fixture JSON, generated assets, docs and vendored sources are outside this scope.
export function authored(path) {
  return /^(ui\/(src\/.*\.tsx?|tests\/.*\.mjs|[^/]+\.(mjs|json))|tests\/ui\/.*\.mjs)$/.test(path);
}
export async function checkFiles(staged = false, write = false) {
  if (staged && write) throw new Error('Staged checks are read-only');
  const args = staged
    ? ['diff', '--cached', '--name-only', '--diff-filter=ACMR', '-z']
    : ['ls-files', '-z'];
  const paths = execFileSync('git', args, { cwd: root }).toString().split('\0').filter(authored);
  const config = staged
    ? JSON.parse(execFileSync('git', ['show', ':.prettierrc.json'], { cwd: root }).toString())
    : await prettier.resolveConfig(resolve(root, '.prettierrc.json'));

  let ok = true;
  for (const path of paths) {
    // Read the index, never the working tree, when checking a partially staged file.
    const source = staged
      ? execFileSync('git', ['show', ':' + path], { cwd: root }).toString()
      : await readFile(resolve(root, path), 'utf8');
    const options = { ...config, filepath: path };
    if (write) {
      const { writeFile } = await import('node:fs/promises');
      await writeFile(resolve(root, path), await prettier.format(source, options));
    } else if (!(await prettier.check(source, options))) {
      console.error(
        `${path}: formatting differs; run npm --prefix ui run format, then stage the intended changes.`,
      );
      ok = false;
    }
  }
  return ok;
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exitCode = (await checkFiles(
    process.argv.includes('--staged'),
    process.argv.includes('--write'),
  ))
    ? 0
    : 1;
}
