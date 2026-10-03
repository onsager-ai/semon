import type { ComponentChildren } from 'preact';
/** Log text is parsed into elements, never interpreted as HTML. */
export function externalUrl(value: unknown): value is string {
  if (typeof value !== 'string' || !/^https?:\/\//i.test(value)) return false;
  try {
    return /^https?:$/.test(new URL(value).protocol);
  } catch {
    return false;
  }
}
function Link({ text, url }: { text: string; url: string }) {
  return externalUrl(url) ? (
    <a href={url} target="_blank" rel="noopener noreferrer">
      {text}
    </a>
  ) : (
    <>{text}</>
  );
}
export function Inline({ text }: { text: string }): ComponentChildren {
  const nodes: ComponentChildren[] = [];
  const re =
    /(`[^`]+`|\*\*(?:`[^`\n]*`|[^*`\n])*?(?:\*\*|…\s*$)|~~[^~\n]+~~|\[[^\]\n]+\]\((?:[^()\s]|\([^()\s]*\))+\)|https?:\/\/[^\s<>`)\]]+|(?<![\w*])\*[^*\s](?:[^*\n]*[^*\s])?\*(?![\w*]))/g;
  let last = 0,
    match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    let token = match[0];
    if (token[0] === '`') nodes.push(<code>{token.slice(1, -1)}</code>);
    else if (token.startsWith('**'))
      nodes.push(
        <strong>
          <Inline text={token.endsWith('**') ? token.slice(2, -2) : token.slice(2)} />
        </strong>,
      );
    else if (token.startsWith('~~'))
      nodes.push(
        <del>
          <Inline text={token.slice(2, -2)} />
        </del>,
      );
    else if (token[0] === '[') {
      const k = token.indexOf('](');
      nodes.push(<Link text={token.slice(1, k)} url={token.slice(k + 2, -1)} />);
    } else if (token[0] === '*')
      nodes.push(
        <em>
          <Inline text={token.slice(1, -1)} />
        </em>,
      );
    else {
      token = token.replace(/[.,;:!?'"…]+$/, '');
      nodes.push(<Link text={token} url={token} />);
      re.lastIndex = match.index + token.length;
    }
    last = match.index + token.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

import { render } from 'preact';
type Block =
  | { kind: 'paragraph' | 'heading'; text: string; level?: number }
  | { kind: 'rule' }
  | { kind: 'code'; text: string; language: string }
  | { kind: 'table'; head: string[]; rows: string[][] }
  | { kind: 'quote'; lines: string[] }
  | { kind: 'list'; ordered: boolean; start: number; items: ListItem[] };
interface ListItem {
  lines: string[];
  children: Block[];
}
const cells = (line: string) =>
  line
    .trim()
    .replace(/^\|/, '')
    .replace(/\|$/, '')
    .split(/(?<!\\)\|/)
    .map((c) => c.trim().replace(/\\\|/g, '|'));
const separator = (line: string) =>
  /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*…?\s*$/.test(line) && line.includes('-');
function blocks(text: string): Block[] {
  const output: Block[] = [],
    lines = text.split('\n');
  let lists: { indent: number; block: Extract<Block, { kind: 'list' }> }[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i],
      fence = /^\s*(```|~~~)\s*([\w+#.-]*)/.exec(line);
    if (fence) {
      lists = [];
      const code: string[] = [];
      for (i++; i < lines.length && !lines[i].trimStart().startsWith(fence[1]); i++)
        code.push(lines[i]);
      output.push({ kind: 'code', text: code.join('\n'), language: fence[2] });
      continue;
    }
    if (/^\s*\|/.test(line) && separator(lines[i + 1] ?? '')) {
      lists = [];
      const rows: string[][] = [];
      for (i += 2; i < lines.length && /^\s*\|/.test(lines[i]); i++) rows.push(cells(lines[i]));
      output.push({ kind: 'table', head: cells(line), rows });
      i--;
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      lists = [];
      output.push({ kind: 'rule' });
      continue;
    }
    const item = /^(\s*)([-*]|\d{1,3}[.)])\s+(.*)$/.exec(line);
    if (item) {
      const indent = item[1].replace(/\t/g, '    ').length,
        ordered = /\d/.test(item[2]);
      while (
        lists.length &&
        (indent < lists.at(-1)!.indent ||
          (indent === lists.at(-1)!.indent && lists.at(-1)!.block.ordered !== ordered))
      )
        lists.pop();
      let top = lists.at(-1);
      if (!top || indent > top.indent) {
        const block: Extract<Block, { kind: 'list' }> = {
          kind: 'list',
          ordered,
          start: ordered ? parseInt(item[2], 10) : 1,
          items: [],
        };
        const parent = top?.block.items.at(-1);
        (parent ? parent.children : output).push(block);
        lists.push((top = { indent, block }));
      }
      top.block.items.push({ lines: [item[3]], children: [] });
      continue;
    }
    if (lists.length && /^\s+\S/.test(line)) {
      lists.at(-1)!.block.items.at(-1)!.lines.push(line.trim());
      continue;
    }
    lists = [];
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      output.push({ kind: 'heading', text: heading[2], level: heading[1].length });
      continue;
    }
    if (/^\s*>/.test(line)) {
      const quote: string[] = [];
      for (; i < lines.length && /^\s*>/.test(lines[i]); i++)
        quote.push(lines[i].replace(/^\s*>\s?/, ''));
      output.push({ kind: 'quote', lines: quote });
      i--;
      continue;
    }
    output.push({ kind: 'paragraph', text: line });
  }
  return output;
}
function Blocks({ items }: { items: readonly Block[] }): ComponentChildren {
  return items.map((block, i) => {
    switch (block.kind) {
      case 'paragraph':
        return (
          <p key={i}>
            <Inline text={block.text} />
          </p>
        );
      case 'heading':
        return (
          <p
            key={i}
            class={'mh mh' + Math.min(block.level ?? 1, 3)}
            role="heading"
            aria-level={Math.min(6, (block.level ?? 1) + 2)}
          >
            <Inline text={block.text} />
          </p>
        );
      case 'rule':
        return <hr key={i} />;
      case 'code':
        return (
          <div
            key={i}
            class="codeblock"
            tabIndex={0}
            role="region"
            aria-label={(block.language ? block.language + ' ' : '') + 'code'}
          >
            <pre>{block.text}</pre>
          </div>
        );
      case 'table':
        return (
          <div key={i} class="tbl" tabIndex={0} role="region" aria-label="Table">
            <table>
              <thead>
                <tr>
                  {block.head.map((cell, j) => (
                    <th key={j}>
                      <Inline text={cell} />
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {block.rows.map((row, j) => (
                  <tr key={j}>
                    {row.map((cell, k) => (
                      <td key={k}>
                        <Inline text={cell} />
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      case 'quote':
        return (
          <blockquote key={i}>
            {block.lines.map((line, j) => (
              <p key={j}>
                <Inline text={line} />
              </p>
            ))}
          </blockquote>
        );
      case 'list': {
        const children = block.items.map((item, j) => (
          <li key={j}>
            {item.lines.map((line, k) => (
              <>
                {k > 0 && <br />}
                <Inline text={line} />
              </>
            ))}
            <Blocks items={item.children} />
          </li>
        ));
        return block.ordered ? (
          <ol key={i} start={block.start === 1 ? undefined : block.start}>
            {children}
          </ol>
        ) : (
          <ul key={i}>{children}</ul>
        );
      }
    }
  });
}
export function Markdown({ text, className = 'body' }: { text: string; className?: string }) {
  return (
    <div class={className + ' md'}>
      <Blocks items={blocks(text)} />
    </div>
  );
}
export function createMarkdown(text: string, className = 'body'): HTMLDivElement {
  const root = document.createElement('div');
  root.className = className + ' md';
  render(<Blocks items={blocks(text)} />, root);
  return root;
}
export function createRich(
  tag: keyof HTMLElementTagNameMap,
  className: string | null,
  text: string,
): HTMLElement {
  const root = document.createElement(tag);
  if (className) root.className = className;
  render(<Inline text={text} />, root);
  return root;
}

export function MarkdownContent({ text }: { text: string }) {
  return <Blocks items={blocks(text)} />;
}
