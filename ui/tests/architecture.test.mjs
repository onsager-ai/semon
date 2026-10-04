import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { checkArchitecture, checkSources } from '../architecture-check.mjs';
const root = path.resolve('architecture-fixture/src');
const files = (entries) =>
  new Map(Object.entries(entries).map(([name, text]) => [path.join(root, name), text]));

test(
  'current viewer respects composition ownership and has no runtime import cycles',
  checkSources,
);

test('composition accepts mount construction, but rejects feature imports of every kind', () => {
  checkArchitecture(
    files({
      'app/composition.ts': 'export class ViewerComposition {}',
      'app/viewer.ts':
        "import { ViewerComposition } from './composition'; new ViewerComposition();",
    }),
  );
  for (const source of [
    "import type { ViewerComposition } from './composition';",
    "import { ViewerComposition } from './composition.js'; new ViewerComposition();",
    "export * from './composition';",
    "export type { ViewerComposition } from './composition';",
    "type Host = import('./composition').ViewerComposition;",
    "const owner = import('./composition');",
    "const owner = require('./composition');",
    "import Owner = require('./composition');",
  ]) {
    assert.throws(
      () =>
        checkArchitecture(
          files({
            'app/composition.ts': 'export class ViewerComposition {}',
            'app/feature.ts': source,
          }),
        ),
      /only app\/viewer.ts may import composition/,
    );
  }
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/composition.ts': 'export class ViewerComposition {}',
          'app/viewer.ts': "export * from './composition';",
        }),
      ),
    /must not be re-exported/,
  );
});

test('TypeScript path aliases cannot conceal composition dependencies', () => {
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/composition.ts': 'export class ViewerComposition {}',
          'app/feature.ts': "import type { ViewerComposition } from '@owner';",
        }),
        { baseUrl: root, paths: { '@owner': ['app/composition.ts'] } },
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

test('local mount re-exports and JavaScript barrels cannot expose composition', () => {
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/composition.ts': 'export class ViewerComposition {}',
          'app/viewer.ts':
            "import { ViewerComposition as Owner } from './composition'; export { Owner };",
        }),
      ),
    /composition must not be re-exported/,
  );
  assert.throws(
    () =>
      checkArchitecture(
        files({
          'app/composition.ts': 'export class ViewerComposition {}',
          'app/barrel.js': "export * from './composition';",
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
          'app/composition.ts': 'export class ViewerComposition {}',
          'app/feature.d.ts': "import type { ViewerComposition } from './composition';",
        }),
      ),
    /only app\/viewer.ts/,
  );
});
