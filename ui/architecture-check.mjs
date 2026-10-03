import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const normalize = (name) => path.resolve(name).replaceAll('\\', '/');

/** Check authored modules, using TypeScript resolution and emitted runtime imports. */
export function checkArchitecture(input, options = {}) {
  const sources = new Map([...input].map(([name, source]) => [normalize(name), source]));
  const directories = new Set();
  for (const name of sources.keys()) {
    for (let dir = path.dirname(name); !directories.has(dir); dir = path.dirname(dir)) {
      directories.add(dir);
      if (dir === path.dirname(dir)) break;
    }
  }
  const compilerOptions = {
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    target: ts.ScriptTarget.ES2022,
    ...options,
  };
  const resolver = {
    fileExists: (name) => sources.has(normalize(name)),
    readFile: (name) => sources.get(normalize(name)),
    directoryExists: (name) => directories.has(normalize(name)),
    getCurrentDirectory: () => process.cwd(),
  };
  const graph = new Map([...sources.keys()].map((name) => [name, new Set()]));
  function modules(source, name, accept) {
    const ast = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true);
    function visit(node) {
      if (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) {
        if (node.moduleSpecifier) accept(node.moduleSpecifier.text, node);
      } else if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument)) {
        accept(node.argument.literal.text, node);
      } else if (
        ts.isImportEqualsDeclaration(node) &&
        ts.isExternalModuleReference(node.moduleReference)
      ) {
        accept(node.moduleReference.expression.text, node);
      } else if (
        ts.isCallExpression(node) &&
        (node.expression.kind === ts.SyntaxKind.ImportKeyword ||
          (ts.isIdentifier(node.expression) && node.expression.text === 'require'))
      ) {
        const specifier = node.arguments[0];
        if (!specifier || !ts.isStringLiteralLike(specifier)) {
          throw new Error(`${name}: architecture requires literal module paths`);
        }
        accept(specifier.text, node);
      }
      ts.forEachChild(node, visit);
    }
    visit(ast);
  }
  for (const [name, source] of sources) {
    const resolve = (specifier) => {
      const resolved = ts.resolveModuleName(
        specifier,
        name,
        compilerOptions,
        resolver,
      ).resolvedModule;
      return resolved && normalize(resolved.resolvedFileName);
    };
    const compositionBindings = new Set();
    // Type-only imports also couple features to composition. Only its mount owner may import it.
    modules(source, name, (specifier, node) => {
      const target = resolve(specifier);
      if (target?.endsWith('/app/composition.ts')) {
        const mount = normalize(path.join(path.dirname(target), 'viewer.ts'));
        if (name !== mount || ts.isExportDeclaration(node)) {
          throw new Error(
            `${name}: only app/viewer.ts may import composition; it must not be re-exported`,
          );
        }
        if (ts.isImportDeclaration(node) && node.importClause) {
          const clause = node.importClause;
          if (clause.name) compositionBindings.add(clause.name.text);
          if (clause.namedBindings) {
            if (ts.isNamespaceImport(clause.namedBindings))
              compositionBindings.add(clause.namedBindings.name.text);
            else
              for (const binding of clause.namedBindings.elements)
                compositionBindings.add(binding.name.text);
          }
        }
      }
    });
    const ast = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true);
    for (const statement of ast.statements) {
      if (
        ts.isExportDeclaration(statement) &&
        !statement.moduleSpecifier &&
        statement.exportClause &&
        ts.isNamedExports(statement.exportClause) &&
        statement.exportClause.elements.some((binding) =>
          compositionBindings.has((binding.propertyName ?? binding.name).text),
        )
      ) {
        throw new Error(`${name}: composition must not be re-exported`);
      }
    }
    if (/\.d\.[cm]?ts$/.test(name)) continue;
    const emitted = ts.transpileModule(source, { fileName: name, compilerOptions }).outputText;
    modules(emitted, name.replace(/\.tsx?$/, '.js'), (specifier) => {
      const target = resolve(specifier);
      if (graph.has(target)) graph.get(name).add(target);
    });
  }
  const active = new Set(),
    visited = new Set(),
    stack = [];
  function visit(name) {
    if (active.has(name)) {
      throw new Error(
        `Runtime import cycle: ${[...stack.slice(stack.indexOf(name)), name].join(' -> ')}`,
      );
    }
    if (visited.has(name)) return;
    active.add(name);
    stack.push(name);
    for (const dependency of graph.get(name)) visit(dependency);
    stack.pop();
    active.delete(name);
    visited.add(name);
  }
  for (const name of graph.keys()) visit(name);
}

export async function checkSources() {
  const root = fileURLToPath(new URL('./src/', import.meta.url));
  const sources = new Map();
  async function walk(dir) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const name = path.join(dir, entry.name);
      if (entry.isDirectory()) await walk(name);
      else if (/\.(?:[cm]?[jt]s|[jt]sx)$/.test(name))
        sources.set(name, await readFile(name, 'utf8'));
    }
  }
  await walk(root);
  const config = ts.readConfigFile(
    fileURLToPath(new URL('./tsconfig.json', import.meta.url)),
    ts.sys.readFile,
  );
  if (config.error)
    throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
  const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, path.dirname(root));
  if (parsed.errors.length)
    throw new Error(ts.flattenDiagnosticMessageText(parsed.errors[0].messageText, '\n'));
  checkArchitecture(sources, parsed.options);
}
if (process.argv[1] && normalize(process.argv[1]) === normalize(fileURLToPath(import.meta.url))) {
  await checkSources();
}
