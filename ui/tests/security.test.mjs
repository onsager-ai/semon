import { test } from 'node:test';
import assert from 'node:assert/strict';
import { checkSource, checkSources } from '../security-check.mjs';
test('all application sources enforce the HTML/style boundary', checkSources);
test('HTML sinks, inline style, unsafe prop spreads and eval are rejected', () => {
  for (const source of [
    'const value: any = input', '// @ts-nocheck\nconst x = 1', '// @ts-ignore\nconst x = 1', '// @ts-expect-error\nconst x = 1',
    'node.innerHTML = text', 'node["outerHTML"] = text', 'node.insertAdjacentHTML("beforeend", text)',
    'document.write(text)', 'eval(text)', 'new Function(text)', 'node.style.cssText = text',
    '<div dangerouslySetInnerHTML={{__html: text}} />', '<div style={{color:"red"}} />',
    '<div {...untrusted} />', '<div title="native" />', 'node.setAttribute("style", text)',
  ]) assert.throws(() => checkSource(source, 'hostile.tsx'), /forbidden application boundary/);
});
