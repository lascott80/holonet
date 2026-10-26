import test from 'node:test';
import assert from 'node:assert/strict';
import {initialGroups,initialSettings,defaultBindings} from '../src/data.js';
const defaults={version:1,hosts:[],groups:initialGroups,keyMetadata:[],settings:initialSettings,bindings:defaultBindings,sessionTabs:[]};
test('rapid change then revert persists the final value; failed writes retry on close',async()=>{
 let fail=false;const writes=[];
 globalThis.window={holonetDesktop:{save:async state=>{await new Promise(resolve=>setTimeout(resolve,2));if(fail)throw new Error('disk unavailable');writes.push(structuredClone(state))}}};
 const {saveState,flushState}=await import('../src/persistence.js');
 await saveState(defaults);
 await Promise.all([saveState({...defaults,settings:{...initialSettings,fontSize:17}}),saveState(defaults)]);
 assert.equal(writes.at(-1).settings.fontSize,15);
 assert.deepEqual(writes.map(s=>s.settings.fontSize),[15,17,15]);
 fail=true;await assert.rejects(saveState({...defaults,settings:{...initialSettings,fontSize:18}}),/disk unavailable/);
 fail=false;await flushState();assert.equal(writes.at(-1).settings.fontSize,18);
 delete globalThis.window;
});
