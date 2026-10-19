import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const output = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook/census');
const inventory = JSON.parse(fs.readFileSync(path.join(here, 'storybook-capture-owners.json'), 'utf8'));
const sourceExtensions = new Set(['.js', '.jsx', '.mjs', '.cjs', '.ts', '.tsx', '.mts', '.cts']);
const ignoredDirectories = new Set(['node_modules', 'out', 'storybook-static', 'dist', 'vendor', 'generated', '.git']);
const files = [];
function visit(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (entry.isDirectory()) {
      if (!ignoredDirectories.has(entry.name)) visit(path.join(directory, entry.name));
    } else if (entry.isFile() && sourceExtensions.has(path.extname(entry.name))) files.push(path.join(directory, entry.name));
  }
}
for (const directory of ['ui', 'tests/ui']) visit(path.join(root, directory));
const findings = [];
const lineAt = (source, index) => source.slice(0, index).split('\n').length;
const classifyMaskValue = (value) => /^\s*(?:\[\s*(?:(?:['"][^'"]*['"])(?:\s*,\s*['"][^'"]*['"])*\s*,?)?\s*\]|['"][^'"]*['"]|(?:true|false|null|\d+))\s*[,}]/.test(value)
  ? 'literal-static' : 'dynamic-expression';
for (const file of files) {
  const relative = path.relative(root, file).split(path.sep).join('/');
  const source = fs.readFileSync(file, 'utf8');
  const seen = new Set();
  for (const pattern of [/(?:\.|\b)\s*screenshot\s*\(/g, /\bmask\s*:/g, /\bmaskColor\s*:/g, /\bmasks\s*:/g, /\b(?:captureStableFrame|captureFrame|regionShot|capture)\s*\(/g, /(?:import[\s\S]{0,180}?from\s*|require\s*\()\s*['"][^'"]*(?:capture|visual)[^'"]*['"]/g]) {
    for (const match of source.matchAll(pattern)) {
      const kind = /maskColor\s*:/.test(match[0]) ? 'playwright-mask-color-option' : /masks\s*:/.test(match[0]) ? 'masks-metadata' : /mask\s*:/.test(match[0]) ? 'playwright-mask-option' : /screenshot\s*\(/.test(match[0]) ? 'screenshot-call' : /import|require/.test(match[0]) ? 'shared-helper-import' : 'shared-capture-call';
      const key = `${kind}:${match.index}`;
      if (seen.has(key)) continue;
      seen.add(key);
      const excerpt = source.slice(match.index, Math.min(source.length, match.index + 100)).split('\n')[0].trim();
      const finding = { owner: relative, line: lineAt(source, match.index), kind, excerpt };
      if (kind === 'playwright-mask-option' || kind === 'playwright-mask-color-option') finding.maskClass = classifyMaskValue(source.slice(match.index + match[0].length, match.index + match[0].length + 160));
      findings.push(finding);
    }
  }
}
findings.sort((a, b) => a.owner.localeCompare(b.owner) || a.line - b.line || a.kind.localeCompare(b.kind));
const records = Array.isArray(inventory.records) ? inventory.records : [];
const byOwner = new Map();
const errors = [];
for (const record of records) {
  if (!record.owner || byOwner.has(record.owner)) errors.push(`duplicate or missing inventory owner: ${record.owner ?? '(empty)'}`);
  else byOwner.set(record.owner, record);
  if (!record.status || !record.disposition || !Object.hasOwn(record, 'successor') || !record.evidence || !Array.isArray(record.maskOptions)) errors.push(`incomplete owner disposition: ${record.owner ?? '(empty)'}`);
}
const observedOwners = new Set(findings.map((entry) => entry.owner));
for (const owner of observedOwners) if (!byOwner.has(owner)) errors.push(`unclassified owner ${owner}`);
for (const owner of byOwner.keys()) if (!observedOwners.has(owner)) errors.push(`orphan inventory declaration ${owner}`);
for (const finding of findings.filter((entry) => entry.kind === 'playwright-mask-option' || entry.kind === 'playwright-mask-color-option')) {
  const record = byOwner.get(finding.owner);
  const declared = record?.maskOptions?.some((option) => option.line === finding.line && option.classification === finding.maskClass);
  if (!declared) errors.push(`unclassified ${finding.maskClass} mask option ${finding.owner}:${finding.line}`);
}
const evidence = {
  version: 1,
  method: inventory.method,
  scope: ['ui', 'tests/ui'],
  exclusions: [...ignoredDirectories].sort(),
  limitations: ['Lexical discovery only; dynamic options are not evaluated.', 'A named call or mask property is an owner signal, not proof of runtime behavior.', 'Shared-helper imports and indirect runtime call graphs may require explicit review.', 'No images, browsers, or application runtime are read or invoked.'],
  findings,
  inventoryErrors: errors,
};
fs.mkdirSync(output, { recursive: true });
fs.writeFileSync(path.join(output, 'owners.json'), JSON.stringify(evidence, null, 2) + '\n');
const markdown = ['# Storybook capture and mask owner census', '', `Method: ${inventory.method}`, '', ...evidence.limitations.map((item) => `- ${item}`), '', '## Findings', '', '| Owner | Line | Kind | Mask classification | Call or declaration |', '|---|---:|---|---|---|', ...findings.map((item) => `| \`${item.owner}\` | ${item.line} | ${item.kind} | ${item.maskClass ?? ''} | \`${item.excerpt.replaceAll('|', '\\|')}\` |`), '', '## Inventory errors', '', ...(errors.length ? errors.map((error) => `- ${error}`) : ['None']), ''].join('\n');
fs.writeFileSync(path.join(output, 'owners.md'), markdown);
if (errors.length) throw new Error(`Storybook capture census found ${errors.length} inventory error(s); see ${path.relative(root, path.join(output, 'owners.md'))}`);
console.log(`Storybook capture census: ${findings.length} findings across ${observedOwners.size} classified owners`);
