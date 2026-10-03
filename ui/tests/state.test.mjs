import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  absWorkingDir: new URL('..', import.meta.url).pathname,
  stdin: { contents: "export * from './src/lib/model'; export * from './src/lib/routes'; export * from './src/lib/live';", resolveDir: new URL('..', import.meta.url).pathname },
  bundle: true, write: false, format: 'esm', platform: 'node',
});
const { ModelStore, parseModel, TranscriptCache, parseRoute, routeUrl, createPagingStore } = await import('data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64'));
const model = () => ({now:1,version:'v1',machine:{id:'m',name:'Machine',up:true},sessions:{s:{name:'Session',harness:'codex',state:'idle',machine:'m',start:1,last:1}},handoffs:[],turns:[{id:'t',sid:'s',sent:[]}]});
test('delta snapshots are isolated from host normalization; rejected bases leave the snapshot usable', () => {
  const store=new ModelStore(), host=store.adopt(model()); host.sessions.s.name='Host mutation';
  const delta={delta:1,from:'v1',version:'v2',collections:{turns:{set:{u:{id:'u',sid:'s',sent:[]}},order:['u','t']}}};
  assert.throws(() => store.apply({...delta,from:'expired'}), /base expired/);
  const next=store.apply(delta); assert.equal(next.sessions.s.name,'Session'); assert.deepEqual(next.turns.map(t=>t.id),['u','t']);
  assert.throws(() => store.apply({...delta,collections:{turns:{order:['missing']}}}), /Incomplete/);
  for (const order of [[], ['u'], ['u','u'], ['u','t','t']]) assert.throws(() => store.apply({...delta,collections:{turns:{set:delta.collections.turns.set,order}}}), /Incomplete/);
  assert.deepEqual(store.apply(delta).turns.map(t=>t.id),['u','t']);
  assert.throws(() => parseModel({...model(),now:Infinity}), /Invalid/);
  assert.throws(() => parseModel({...model(),turns:[{id:'t',sid:'s',sent:[3]}]}), /Invalid/);
});
test('transcript cache enforces both bounds and refreshes admission order', () => {
  const cache=new TranscriptCache(2,100); cache.keep('a',[{text:'one'}],{});cache.keep('b',[{text:'two'}],{});cache.keep('a',[{text:'new'}],{});cache.keep('c',[{text:'three'}],{});
  assert.deepEqual([...cache.keys()],['a','c']);cache.keep('oversized',[{text:'x'.repeat(100)}],{});assert.equal(cache.has('oversized'),false);
  cache.keep('large',[{text:'x'.repeat(46)}],{});assert.deepEqual([...cache.keys()],['large']);
});
test('routes retain harness URLs, deep-link ownership and the Hub Machines path', () => {
  const api={session:id=>id==='s'?{harness:'codex'}:undefined,machine:id=>id==='m',turn:id=>id==='t'?{id:'t',sid:'s'}:id==='foreign'?{id:'foreign',sid:'other'}:undefined,machinesPath:'/workspace/machines'};
  assert.equal(routeUrl({v:'session',id:'s',turn:'t'},api),'/s/codex/s?turn=t');
  assert.deepEqual(parseRoute({pathname:'/s/codex/s',search:'',hash:'#t#slot:1'},api),{v:'session',id:'s',turn:'t'});
  assert.deepEqual(parseRoute({pathname:'/s/codex/s',search:'?turn=t',hash:''},api),{v:'session',id:'s',turn:'t'});
  assert.deepEqual(parseRoute({pathname:'/trace/codex/s/t',search:'',hash:''},api),{v:'trace',sid:'s',turn:'t'});
  for (const turn of ['foreign','missing']) {
    assert.deepEqual(parseRoute({pathname:'/s/codex/s',search:'?turn='+turn,hash:'#t'},api),{v:'session',id:'s'});
    assert.deepEqual(parseRoute({pathname:'/s/codex/s',search:'',hash:'#'+turn},api),{v:'session',id:'s'});
    assert.deepEqual(parseRoute({pathname:'/trace/codex/s/'+turn,search:'',hash:''},api),{v:'home'});
  }
  assert.deepEqual(parseRoute({pathname:'/workspace/machines',search:'',hash:''},api),{v:'machines'});
  assert.deepEqual(parseRoute({pathname:'/s/codex/missing',search:'',hash:''},api),{v:'home'});
});
test('clearing a transcript aborts both page directions and creates fresh state', () => {
  const store=createPagingStore(), before=store.get('s','before'),after=store.get('s','after');before.controller=new AbortController();after.controller=new AbortController();
  store.clear('s');assert.equal(before.controller.signal.aborted,true);assert.equal(after.controller.signal.aborted,true);assert.notEqual(store.get('s','before'),before);store.destroy();assert.equal(store.states.size,0);
});
