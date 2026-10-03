import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import postcss from 'postcss';
import valueParser from 'postcss-value-parser';

const ui = path.dirname(fileURLToPath(import.meta.url));
const namedColors = new Set(
  'aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen'.split(
    ' ',
  ),
);
export const sharedRoot = path.resolve(ui, '..');
export const sharedFiles = ['viewer.css', 'shell.css', 'select.css'].map(
  (name) => 'crates/semon-sessions/src/' + name,
);
export const readSources = (root, files) =>
  files.map((file) => ({ file, css: fs.readFileSync(path.resolve(root, file), 'utf8') }));
export const sharedPolicy = JSON.parse(
  fs.readFileSync(path.join(ui, 'design-policy.json'), 'utf8'),
);

// Parse CSS and values rather than scanning comments or matching substrings.
// Runtime geometry names require an explicit contract, even when a fallback exists.
export function checkDesign(sources, policy, consumer = {}) {
  const roots = sources.map((source) => ({
    ...source,
    root: postcss.parse(source.css, { from: source.file }),
  }));
  const defined = new Set(Object.keys(policy.runtimeTokens));
  const sharedDefined = new Set();
  for (const { root } of roots)
    root.walkDecls((decl) => {
      if (decl.prop.startsWith('--')) defined.add(decl.prop);
    });
  for (const { file, root } of roots)
    if (sharedFiles.includes(file))
      root.walkDecls((decl) => {
        if (decl.prop.startsWith('--')) sharedDefined.add(decl.prop);
      });
  const failures = [];
  const error = (file, decl, message) =>
    failures.push(
      `${file}:${decl.source.start.line}: ${decl.parent.selector ?? decl.parent.name}: ${message}`,
    );
  const allowed = new Set(policy.approvedLiterals);
  const exceptions = [...policy.exceptions, ...(consumer.exceptions ?? [])];
  const used = new Set();
  for (const { file, root } of roots)
    root.walkDecls((decl) => {
      const signature = [
        file,
        decl.parent.selector ?? '@' + decl.parent.name,
        decl.prop,
        decl.value,
      ].join('|');
      const exception = exceptions.find((entry) => entry.declaration === signature);
      if (exception) {
        used.add(exception);
        return;
      }
      const parsed = valueParser(decl.value);
      parsed.walk((node) => {
        if (node.type === 'function' && node.value.toLowerCase() === 'var') {
          const name = node.nodes.find((item) => item.type === 'word')?.value;
          if (!name || !defined.has(name))
            error(file, decl, `unresolved token ${name ?? '(empty)'}`);
        }
      });
      if (
        (decl.prop.startsWith('--') && sharedFiles.includes(file)) ||
        (decl.parent.type === 'atrule' && decl.parent.name === 'font-face')
      )
        return;
      if (decl.prop.startsWith('--') && sharedDefined.has(decl.prop))
        error(file, decl, `consumer must not redefine shared token ${decl.prop}`);
      if (decl.prop === 'color' && /var\(\s*--faint\s*\)/.test(decl.value))
        error(
          file,
          decl,
          '--faint is reserved for icons and separators; document an exact exception',
        );
      if (/^(font|font-size)$/.test(decl.prop)) {
        parsed.walk((node) => {
          if (node.type === 'function') return false; // calc/min/max require rendered-size verification
          if (
            node.type === 'word' &&
            /^\d*\.?\d+px$/.test(node.value) &&
            Number.parseFloat(node.value) < 12
          )
            error(file, decl, `text size ${node.value} is below 12px`);
        });
      }
      if (decl.prop === 'font')
        parsed.walk((node) => {
          if (node.type === 'function') return false;
          if (
            node.type === 'string' ||
            (node.type === 'word' &&
              !/^(?:[\d.]+(?:px|em|rem|%|deg)?|normal|bold|bolder|lighter|italic|oblique|small-caps|inherit|initial|unset|revert)$/.test(
                node.value,
              ))
          )
            error(file, decl, `font literal ${node.value} must use a role token`);
        });
      if (
        decl.prop === 'font-family' &&
        !/^var\(--(?:sans|mono)\)$|^(?:inherit|initial|unset)$/.test(decl.value)
      )
        error(file, decl, 'font family must use --sans or --mono');
      if (/^(font|font-size)$/.test(decl.prop)) {
        parsed.walk((node) => {
          if (node.type === 'function' && node.value === 'var') return false;
          if (
            node.type === 'word' &&
            /^\d*\.?\d+(?:px|rem|em)$/.test(node.value) &&
            !allowed.has('type:' + node.value)
          )
            error(file, decl, `unapproved typography literal ${node.value}; use a role token`);
        });
      }
      // Color literals anywhere (including shadows/borders) must be tokens. Shared
      // neutral alpha mixes retain their approved values; consumer literals need
      // a narrowly scoped, reasoned exception.
      parsed.walk((node) => {
        if (
          node.type === 'function' &&
          /^(?:rgb|rgba|hsl|hsla|oklch|oklab|lab|lch|hwb|color)$/i.test(node.value)
        ) {
          const literal = valueParser.stringify(node);
          if (!allowed.has('color:' + literal)) error(file, decl, `unapproved color ${literal}`);
          return false;
        }
        if (
          node.type === 'word' &&
          /^#[\da-f]{3,8}$/i.test(node.value) &&
          !allowed.has('color:' + node.value.toLowerCase())
        )
          error(file, decl, `unapproved color ${node.value}`);
        if (node.type === 'word' && namedColors.has(node.value.toLowerCase()))
          error(file, decl, `named color ${node.value} must use a token`);
      });
    });
  for (const entry of exceptions) {
    if (!entry.reason?.trim())
      failures.push('Design exception has no reason: ' + entry.declaration);
    if (!used.has(entry)) failures.push('Stale design exception: ' + entry.declaration);
  }
  return failures;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length && (args.length !== 2 || args[0] !== '--consumer'))
    throw new Error('Usage: node ui/design-check.mjs [--consumer path/to/design-policy.json]');
  let sources = readSources(sharedRoot, sharedFiles),
    consumer = {};
  if (args.length) {
    const configPath = path.resolve(args[1]);
    consumer = JSON.parse(fs.readFileSync(configPath, 'utf8'));
    sources = sources.concat(readSources(path.dirname(configPath), consumer.files));
  }
  const failures = checkDesign(sources, sharedPolicy, consumer);
  for (const failure of failures) console.error(failure);
  console.log(`Design contract: ${sources.length} stylesheets, ${failures.length} violations`);
  if (failures.length) process.exitCode = 1;
}
