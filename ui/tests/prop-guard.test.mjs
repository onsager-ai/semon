import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  absWorkingDir: new URL('..', import.meta.url).pathname,
  stdin: { contents: "import './src/lib/index'; export { h } from 'preact';", resolveDir: new URL('..', import.meta.url).pathname },
  bundle: true, write: false, format: 'esm', platform: 'node', define: { 'process.env.NODE_ENV': '"production"' },
});
const { h } = await import('data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64'));
test('native VNodes reject HTML/style props, unsafe paths and inline handlers', () => {
  for (const props of [{dangerouslySetInnerHTML:{__html:'<img src=x onerror=alert(1)>'}}, {style:'color:red'}, {style:{}}, {title:'native'}, {href:'javascript:alert(1)'}, {src:'//evil.invalid'}, {action:'/\\evil.invalid'}, {onClick:'alert(1)'}]) {
    assert.throws(() => h('div', props), /Forbidden DOM prop|Unsafe DOM path|Inline DOM handler/);
  }
  assert.doesNotThrow(() => h('a', {href:'/account', onClick:() => {}}, '<script>alert(1)</script>'));
});

test('external log links require a safe protocol and explicit tab isolation', () => {
  assert.doesNotThrow(() => h('a', {href:'https://example.com/path', target:'_blank', rel:'noopener noreferrer'}));
  for (const props of [
    {href:'https://example.com'}, {href:'https://example.com', target:'_blank'},
    {href:'javascript:alert(1)', target:'_blank', rel:'noopener noreferrer'},
    {href:'//example.com', target:'_blank', rel:'noopener noreferrer'},
  ]) assert.throws(() => h('a', props), /Unsafe DOM path/);
  assert.throws(() => h('img', {src:'https://example.com/pixel', target:'_blank', rel:'noopener noreferrer'}), /Unsafe DOM path/);
});
