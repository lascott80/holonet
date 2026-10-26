import test from 'node:test';
import assert from 'node:assert/strict';
import {loadWithTerminalFont,terminalFontFamily} from '../src/terminal-font.js';
test('app bootstrap waits for the terminal face before a terminal can measure fallback text',async()=>{
 let resolveFont,started=false,finished=false,request;
 const fontReady=new Promise(resolve=>{resolveFont=resolve});
 const state={runtime:'desktop',state:{hosts:[]}};
 const bootstrap=loadWithTerminalFont(async()=>{started=true;return state},{load:(...args)=>{request=args;return fontReady}}).then(value=>{finished=true;return value});
 await new Promise(resolve=>setImmediate(resolve));
 assert.equal(started,true);assert.equal(finished,false);assert.deepEqual(request,['15px "JetBrains Mono"','MW0123456789']);
 resolveFont([]);assert.equal(await bootstrap,state);
});
test('font load failure uses a monospace fallback without blocking access to saved hosts',async()=>{
 const saved={runtime:'desktop',state:{hosts:[{id:'saved'}]}};
 assert.equal(await loadWithTerminalFont(async()=>saved,{load:async()=>{throw Error('font unavailable')}}),saved);
 assert.equal(await loadWithTerminalFont(async()=>saved,undefined),saved);
 assert.equal(terminalFontFamily('JetBrains Mono'),'"JetBrains Mono", monospace');assert.equal(terminalFontFamily('monospace'),'monospace');
});
test('font preparation does not hide metadata loading failures',async()=>{
 await assert.rejects(loadWithTerminalFont(async()=>{throw Error('Saved settings unreadable')},{load:async()=>[]}),/Saved settings unreadable/);
});
