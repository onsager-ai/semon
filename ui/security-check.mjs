import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
const sinks = [
  'innerHTML',
  'outerHTML',
  'insertAdjacentHTML',
  'cssText',
  'dangerouslySetInnerHTML',
];
export function checkSource(source, name) {
  const ast = ts.createSourceFile(
    name,
    source,
    ts.ScriptTarget.Latest,
    true,
    name.endsWith('.tsx')
      ? ts.ScriptKind.TSX
      : name.endsWith('.ts')
        ? ts.ScriptKind.TS
        : ts.ScriptKind.JS,
  );
  const fail = (text) => {
    throw new Error(`${name}: forbidden application boundary ${text}`);
  };
  if (/(?:\/\/|\/\*)\s*@ts-(?:ignore|nocheck|expect-error)\b/.test(source))
    fail('type-check suppression');
  function visit(node) {
    if (node.kind === ts.SyntaxKind.AnyKeyword) fail('unchecked any type');
    if (
      (ts.isIdentifier(node) || ts.isStringLiteral(node)) &&
      sinks.includes(node.text) &&
      !(
        name.endsWith('/lib/security.ts') &&
        ts.isStringLiteral(node) &&
        ts.isArrayLiteralExpression(node.parent)
      )
    )
      fail(node.text);
    if (ts.isPropertyAccessExpression(node) && node.name.text === 'style') fail('inline DOM style');
    if (ts.isElementAccessExpression(node) && node.argumentExpression?.text === 'style')
      fail('inline DOM style');
    if (ts.isJsxSpreadAttribute(node)) fail('JSX prop spread');
    if (ts.isJsxAttribute(node) && ['style', 'title'].includes(node.name.getText(ast)))
      fail(node.name.getText(ast));
    if (ts.isCallExpression(node)) {
      const call = node.expression.getText(ast);
      if (['eval', 'Function', 'document.write', 'document.writeln'].includes(call)) fail(call);
      if (
        call.endsWith('.setAttribute') &&
        node.arguments[0] &&
        ['style', 'title'].includes(node.arguments[0].text)
      )
        fail(call);
    }
    if (ts.isNewExpression(node) && node.expression.getText(ast) === 'Function')
      fail('new Function');
    ts.forEachChild(node, visit);
  }
  visit(ast);
}
export async function checkSources() {
  const root = new URL('./src/', import.meta.url);
  async function walk(dir) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const url = new URL(entry.name + (entry.isDirectory() ? '/' : ''), dir);
      if (entry.isDirectory()) await walk(url);
      else {
        const source = await readFile(url, 'utf8');
        checkSource(source, fileURLToPath(url));
        if (fileURLToPath(url).includes('/src/lib/') && /from\s+['"]\.\.\//.test(source))
          throw new Error('lib imports outside lib');
      }
    }
  }
  await walk(root);
}
