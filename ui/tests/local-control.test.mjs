import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';
const { outputFiles } = await build({entryPoints:['src/app/localControl.ts'],absWorkingDir:new URL('..',import.meta.url).pathname,bundle:true,write:false,format:'esm',platform:'node'});
const { createLocalControl } = await import('data:text/javascript;base64,'+Buffer.from(outputFiles[0].contents).toString('base64'));
const model = () => ({thread:'native',generation:'generation',activeTurn:'turn',connected:true,capabilities:{input:true,steer:true,interrupt:true,commandApproval:true,fileApproval:true,questions:true},reason:null,requests:[],actions:{}});
test('typed control owner binds exact targets and blocks blind replay after uncertain delivery',async()=>{
  const oldFetch=globalThis.fetch, oldCrypto=globalThis.crypto;
  Object.defineProperty(globalThis,'crypto',{value:webcrypto,configurable:true});
  const calls=[];
  globalThis.fetch=async(url,options)=>{calls.push({url,options,command:JSON.parse(options.body)});throw Error('transport lost');};
  const owner=createLocalControl({request:()=>new AbortController(),releaseRequest:()=>{}},()=>{});
  try {
    owner.adopt(owner.prepare(model()));owner.view().send('hello');await new Promise(resolve=>setImmediate(resolve));
    assert.equal(calls.length,1);assert.deepEqual([calls[0].command.thread,calls[0].command.generation,calls[0].command.activeTurn],['native','generation','turn']);
    assert.equal(calls[0].options.credentials,'same-origin');assert.equal(owner.view().uncertain,true);
    owner.adopt(owner.prepare({...model(),generation:'next'}));owner.view().send('hello');await new Promise(resolve=>setImmediate(resolve));assert.equal(calls.length,1);
    assert.equal(owner.view('another-session'),undefined);
    globalThis.fetch=async()=>({ok:true,json:async()=>({reconnected:true})});owner.view().reconnect();await new Promise(resolve=>setImmediate(resolve));assert.equal(owner.view().uncertain,false);
  } finally {owner.destroy();globalThis.fetch=oldFetch;Object.defineProperty(globalThis,'crypto',{value:oldCrypto,configurable:true});}
});
test('unsupported or malformed live models never create write capabilities',()=>{
 const owner=createLocalControl({},()=>{});
 assert.equal(owner.prepare(undefined),null);
 for(const invalid of [{}, {...model(),activeTurn:12}, {...model(),capabilities:{}}, {...model(),requests:[{}]}])assert.throws(()=>owner.prepare(invalid));
});
