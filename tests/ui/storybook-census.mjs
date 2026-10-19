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
const classifyMaskValue = (value) => /^\s*(?:\[\s*(?:(?:'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*")(?:\s*,\s*(?:'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*")?)*\s*,?)?\s*\]|\{\s*\}|'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"|#[\da-fA-F]{3,8}\b|(?:rgba?|hsla?)\([^)]*\)|(?:transparent|currentColor)|(?:true|false|null)|\d+(?:\.\d+)?)\s*$/i.test(value)
  ? 'literal-static' : 'dynamic-expression';
const readOptionValue = (source, start) => {
  let quote = '', escaped = false;
  const depth = { '[': 0, '{': 0, '(': 0 };
  for (let index = start; index < source.length; index++) {
    const char = source[index];
    if (quote) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === quote) quote = '';
      continue;
    }
    if (char === '"' || char === "'" || char === '`') { quote = char; continue; }
    if (char in depth) { depth[char]++; continue; }
    if (char === ']' && depth['[']) { depth['[']--; continue; }
    if (char === '}' && depth['{']) { depth['{']--; continue; }
    if (char === ')' && depth['(']) { depth['(']--; continue; }
    if ((char === ',' || char === '}') && !depth['['] && !depth['{'] && !depth['(']) return source.slice(start, index).trim();
  }
  return source.slice(start).trim();
};
const normalizeValue = (value) => {
  let result = '', quote = '', escaped = false, pendingSpace = false;
  for (const char of value.trim()) {
    if (quote) {
      result += char;
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === quote) quote = '';
    } else if (char === '"' || char === "'" || char === '`') {
      if (pendingSpace && result && !/[([{,:]/.test(result.at(-1))) result += ' ';
      pendingSpace = false; quote = char; result += char;
    } else if (/\s/.test(char)) pendingSpace = true;
    else {
      if (pendingSpace && result && !/[([{,:]/.test(result.at(-1)) && !/[)\]},:]/.test(char)) result += ' ';
      pendingSpace = false; result += char;
    }
  }
  return result;
};
for (const file of files) {
  const relative = path.relative(root, file).split(path.sep).join('/');
  const source = fs.readFileSync(file, 'utf8');
  const seen = new Set(), optionOrdinals = new Map();
  for (const pattern of [/(?:\.|\b)\s*screenshot\s*\(/g, /\bmask\s*:/g, /\bmaskColor\s*:/g, /\bmasks\s*:/g, /\b(?:captureStableFrame|captureFrame|regionShot|capture)\s*\(/g, /\bfrom\s*['"][^'\"]*(?:capture|visual)[^'\"]*['"]|\brequire\s*\(\s*['"][^'\"]*(?:capture|visual)[^'\"]*['"]/g]) {
    for (const match of source.matchAll(pattern)) {
      const kind = /maskColor\s*:/.test(match[0]) ? 'playwright-mask-color-option' : /masks\s*:/.test(match[0]) ? 'masks-metadata' : /mask\s*:/.test(match[0]) ? 'playwright-mask-option' : /screenshot\s*\(/.test(match[0]) ? 'screenshot-call' : /from\s*['"]|require/.test(match[0]) ? 'shared-helper-import' : 'shared-capture-call';
      const key = `${kind}:${match.index}`;
      if (seen.has(key)) continue;
      seen.add(key);
      const excerpt = source.slice(match.index, Math.min(source.length, match.index + 100)).split('\n')[0].trim();
      const finding = { owner: relative, line: lineAt(source, match.index), kind, excerpt };
      if (kind === 'playwright-mask-option' || kind === 'playwright-mask-color-option' || kind === 'masks-metadata') {
        const option = kind === 'playwright-mask-color-option' ? 'maskColor' : kind === 'masks-metadata' ? 'masks' : 'mask';
        const ordinal = (optionOrdinals.get(option) ?? 0) + 1;
        optionOrdinals.set(option, ordinal);
        const value = normalizeValue(readOptionValue(source, match.index + match[0].length));
        finding.option = option;
        finding.optionOrdinal = ordinal;
        finding.optionValue = value;
        finding.maskClass = classifyMaskValue(value);
      }
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
for (const finding of findings.filter((entry) => entry.kind === 'playwright-mask-option' || entry.kind === 'playwright-mask-color-option' || entry.kind === 'masks-metadata')) {
  const record = byOwner.get(finding.owner);
  const declared = record?.maskOptions?.some((option) => option.kind === finding.kind && option.ordinal === finding.optionOrdinal && option.value === finding.optionValue && option.classification === finding.maskClass);
  if (!declared) errors.push(`unclassified ${finding.maskClass} mask option ${finding.owner}:${finding.line}`);
}
for (const record of records) {
  const observed = findings.filter((finding) => finding.owner === record.owner && ['playwright-mask-option', 'playwright-mask-color-option', 'masks-metadata'].includes(finding.kind));
  for (const option of record.maskOptions ?? [])
    if (!observed.some((finding) => finding.kind === option.kind && finding.optionOrdinal === option.ordinal && finding.optionValue === option.value && finding.maskClass === option.classification))
      errors.push(`orphan or changed mask option declaration ${record.owner}:${option.kind}#${option.ordinal}`);
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
const markdown = ['# Storybook capture and mask owner census', '', `Method: ${inventory.method}`, '', ...evidence.limitations.map((item) => `- ${item}`), '', '## Findings', '', '| Owner | Line | Kind | Mask binding | Call or declaration |', '|---|---:|---|---|---|', ...findings.map((item) => {
  const binding = item.option ? `${item.option}#${item.optionOrdinal}=${item.optionValue} (${item.maskClass})` : '';
  return `| \`${item.owner}\` | ${item.line} | ${item.kind} | ${binding.replaceAll('|', '\\|')} | \`${item.excerpt.replaceAll('|', '\\|')}\` |`;
}), '', '## Inventory errors', '', ...(errors.length ? errors.map((error) => `- ${error}`) : ['None']), ''].join('\n');
fs.writeFileSync(path.join(output, 'owners.md'), markdown);
if (errors.length) throw new Error(`Storybook capture census found ${errors.length} inventory error(s); see ${path.relative(root, path.join(output, 'owners.md'))}`);
console.log(`Storybook capture census: ${findings.length} findings across ${observedOwners.size} classified owners`);
