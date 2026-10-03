import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({ stdin: { contents: "export * from './src/domain/normalize'; export * from './src/domain/calculations'; export * from './src/domain/format';", resolveDir: new URL('..', import.meta.url).pathname }, bundle: true, write: false, format: 'esm', platform: 'node' });
const { normalizeModel, createDomain, machineShorts, dur, compactCount } = await import('data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64'));
const session = (name, extra={}) => ({ name, harness:'codex', state:'idle', machine:'m', start:1, last:2, ...extra });
const fixture = () => ({ now:1000,version:'v1',machine:{id:'m',name:'Machine',up:true},sessions:{s:session('Session',{lane:true}),c:session('Child',{state:'work',parent:'s'})},handoffs:[{id:'h',kind:'spawn',from:'s',to:'c',at:1,status:'work',brief:'Check'}, {id:'failed',kind:'relay',from:'s',to:null,target:'missing',at:2,status:'err',brief:'Send'}],turns:[{id:'t',sid:'s',sent:['h','failed'],u:true,text:'Ask'},{id:'child',sid:'c',start:'h',sent:[],last:true}] });
function domain(model=fixture()) { const normalized=normalizeModel(model); return { normalized, domain:createDomain({...normalized,machines:{m:'Machine'},transcriptMeta:{}},()=>1000,new Set()) }; }
test('normalization isolates wire snapshots, resolves failed sends and indexes handoff ownership',()=>{
 const wire=fixture(), before=structuredClone(wire), {normalized,domain:d}=domain(wire);
 assert.deepEqual(wire,before); assert.equal(normalized.handoffs[1].to,'unsent:missing'); assert.equal(normalized.sessions['unsent:missing'].state,'err');
 assert.equal(normalized.holds.get('h'),normalized.turn.get('t')); assert.equal(normalized.starts.get('h'),normalized.turn.get('child'));
 assert.equal(d.traceRoot(normalized.turn.get('child')).id,'t'); assert.equal(d.turnEnd(normalized.turn.get('child')).text,'Still working');
 assert.deepEqual(d.descendantsOf('s',d.sessionChildren()).map(s=>s.id),['c']);
 normalized.sessions.s.name='Changed'; assert.equal(wire.sessions.s.name,'Session');
});
test('hostile optional values fail before domain normalization can commit',()=>{
 for (const extra of [{cost:{usd:Infinity}}, {activity:['run','cmd','age']},{busy:[[1,'end']]},{tokens_by_model:{bad:{input:'<img>'}}},{parent:{}},{signals:{bad:NaN}}]) {
  const model=fixture();model.sessions.s={...model.sessions.s,...extra};assert.throws(()=>normalizeModel(model),/Invalid/);
 }
 const model=fixture();model.handoffs[0].kind='unknown';assert.throws(()=>normalizeModel(model),/Invalid handoff/);
});
test('handoff briefs are required strings before renderable state is adopted',()=>{
 for(const brief of [undefined,null,{},42]) {const model=fixture();model.handoffs[0].brief=brief;assert.throws(()=>normalizeModel(model),/Invalid/);}
 const model=fixture();model.handoffs[0].brief='';assert.equal(normalizeModel(model).handoffs[0].brief,'');
});
test('cyclic ancestry and traces stop; totals preserve unknown prices and zero figures',()=>{
 const model=fixture(); model.sessions.s.parent='c';const {domain:d}=domain(model);
 assert.deepEqual(d.lineageOf('s').map(s=>s.id),['c','s']);assert.deepEqual(d.descendantsOf('s',d.sessionChildren()).map(s=>s.id),['c']);
 const known={...session('Known'),id:'k',cost:{usd:1,by_model:{m:{usd:1,tokens:{input:2}}}}};
 const unknown={...session('Unknown'),id:'u',cost:{usd:null,unpriced_models:['missing']}};
 assert.equal(d.costForSessions([known,unknown]).usd,null);assert.deepEqual(d.costForSessions([known,unknown]).unpriced_models,['missing']);
 assert.equal(d.costForSessions([]).usd,0);assert.equal(d.costText(d.costForSessions([known])),'$1.00');
});
test('machine labels resolve collisions and time/count formatting retains the viewer contract',()=>{
 const labels=machineShorts([['a','build-host-very-long-east.example'],['b','build-host-very-long-west.example']]);assert.notEqual(labels.get('a'),labels.get('b'));
 assert.equal(dur(0,90061000,0),'1d 1h');assert.equal(compactCount(12422228),'12.4M');
});
