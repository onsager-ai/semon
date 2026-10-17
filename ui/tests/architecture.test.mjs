import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { checkArchitecture, checkSources } from '../architecture-check.mjs';
const root = path.resolve('architecture-fixture/src');
const files = (entries) =>
  new Map(Object.entries(entries).map(([name, text]) => [path.join(root, name), text]));

test(
  'current viewer respects canonical reader ownership and has no runtime import cycles',
  checkSources,
);

test('canonical reader accepts mount construction, but rejects feature imports of every kind', () => {
  checkArchitecture(
    files({
      'app/catalogViewer.tsx': 'export class CatalogReader {}',
      'app/viewer.ts': "import { CatalogReader } from './catalogViewer'; new CatalogReader();",
    }),
  );
  for (const source of [
    "import type { CatalogReader } from './catalogViewer';",
    "import { CatalogReader } from './catalogViewer.js'; new CatalogReader();",
    "export * from './catalogViewer';",
    "export type { CatalogReader } from './catalogViewer';",
    "type Host = import('./catalogViewer').CatalogReader;",
    "const owner = import('./catalogViewer');",
    "const owner = require('./catalogViewer');",
    "import Owner = require('./catalogViewer');",
  ]) {
    assert.throws(
      () =>
        checkArchitecture(
          files({
            'app/catalogViewer.tsx': 'export class CatalogReader {}',
            'app/feature.ts': source,
          }),
        ),
      /only app\/viewer.ts may import canonical reader/,
    );
  }
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/catalogViewer.tsx': 'export class CatalogReader {}',
          'app/viewer.ts': "export * from './catalogViewer';",
        }),
      ),
    /must not be re-exported/,
  );
});

test('TypeScript path aliases cannot conceal canonical reader dependencies', () => {
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/catalogViewer.tsx': 'export class CatalogReader {}',
          'app/feature.ts': "import type { CatalogReader } from '@owner';",
        }),
        { baseUrl: root, paths: { '@owner': ['app/catalogViewer.tsx'] } },
      ),
    /only app\/viewer.ts/,
  );
});

test('reports cycles through re-exports, directory indexes and dynamic imports', () => {
  for (const edge of ["export { value } from './b';", "import('./b');", "require('./b');"]) {
    assert.throws(
      () =>
        checkArchitecture(
          files({
            'a.ts': edge,
            'b/index.ts': "import '../a'; export const value = 1;",
          }),
        ),
      /Runtime import cycle: .*a\.ts.*b\/index\.ts.*a\.ts/,
    );
  }
  assert.throws(
    () => checkArchitecture(files({ 'self.ts': "import './self';" })),
    /Runtime import cycle/,
  );
});

test('type-only cycles and erased type references do not become runtime edges', () => {
  checkArchitecture(
    files({
      'a.ts': "import { B } from './b'; export type A = { b: B };",
      'b.ts': "import type { A } from './a'; export type B = { a: A };",
    }),
  );
  checkArchitecture(
    files({
      'a.ts': "export type { B } from './b'; export type A = string; export const value = 1;",
      'b.ts': "import { value, type A } from './a'; console.log(value); export type B = A;",
    }),
  );
});

test('nonliteral module loads cannot bypass the statically checked graph', () => {
  for (const source of ["import('./' + name);", 'require(name);']) {
    assert.throws(
      () => checkArchitecture(files({ 'a.ts': source })),
      /requires literal module paths/,
    );
  }
});

test('local mount re-exports and JavaScript barrels cannot expose canonical reader', () => {
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/catalogViewer.tsx': 'export class CatalogReader {}',
          'app/viewer.ts':
            "import { CatalogReader as Owner } from './catalogViewer'; export { Owner };",
        }),
      ),
    /canonical reader must not be re-exported/,
  );
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/catalogViewer.tsx': 'export class CatalogReader {}',
          'app/barrel.js': "export * from './catalogViewer';",
          'app/feature.ts': "import './barrel.js';",
        }),
      ),
    /only app\/viewer.ts/,
  );
});

test('runtime cycles also resolve TypeScript path aliases', () => {
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'a.ts': "import '@next';",
          'b/index.ts': "import '../a';",
        }),
        { baseUrl: root, paths: { '@next': ['b/index.ts'] } },
      ),
    /Runtime import cycle/,
  );
});

test('declaration files contribute type boundaries without runtime generation', () => {
  checkArchitecture(
    files({
      'a.d.ts': "import type { B } from './b'; export type A = B;",
      'b.ts': "import type { A } from './a'; export type B = { next?: A };",
    }),
  );
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/catalogViewer.tsx': 'export class CatalogReader {}',
          'app/feature.d.ts': "import type { CatalogReader } from './catalogViewer';",
        }),
      ),
    /only app\/viewer.ts/,
  );
});
