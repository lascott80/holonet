import test from 'node:test';
import assert from 'node:assert/strict';
import {editTerminal,clipboardShortcut} from '../src/terminal-edit.js';

test('copy uses terminal selection, and an empty selection preserves clipboard',async()=>{
 let clipboard='original';const bridge={clipboardWrite:async text=>{clipboard=text}};
 await editTerminal({getSelection:()=>''},bridge,'copy');assert.equal(clipboard,'original');
 await editTerminal({getSelection:()=> 'selected\nterminal text'},bridge,'copy');assert.equal(clipboard,'selected\nterminal text');
});
test('paste passes multiline text through the terminal paste pipeline once',async()=>{
 const writes=[];await editTerminal({paste:text=>writes.push(text)},{clipboardRead:async()=> 'echo one\r\necho two\n'},'paste');
 assert.deepEqual(writes,['echo one\r\necho two\n']);
 await editTerminal({paste:text=>writes.push(text)},{clipboardRead:async()=>''},'paste');assert.equal(writes.length,1);
});
test('clipboard failures propagate, and local selection/clear never write to SSH',async()=>{
 await assert.rejects(editTerminal({}, {clipboardRead:async()=>{throw Error('denied')}},'paste'),/denied/);
 const calls=[];const term={selectAll:()=>calls.push('selection'),clear:()=>calls.push('clear')};
 await editTerminal(term,{},'selectAll');await editTerminal(term,{},'clear');assert.deepEqual(calls,['selection','clear']);
});
test('platform clipboard shortcuts never capture plain Ctrl+C or Ctrl+V',()=>{
 const event=(key,mods={})=>({key,metaKey:false,ctrlKey:false,shiftKey:false,altKey:false,...mods});
 for(const platform of ['darwin','win32']){
  assert.equal(clipboardShortcut(event('c',{ctrlKey:true}),platform),false);
  assert.equal(clipboardShortcut(event('v',{ctrlKey:true}),platform),false);
 }
 for(const key of ['c','v','a']){
  assert.equal(clipboardShortcut(event(key,{metaKey:true}),'darwin'),true);
  assert.equal(clipboardShortcut(event(key,{ctrlKey:true,shiftKey:true}),'win32'),true);
 }
 assert.equal(clipboardShortcut(event('c',{ctrlKey:true,shiftKey:true,altKey:true}),'win32'),false);
});

test('Windows app shortcuts use Control while copy/paste keep Ctrl+C free',async()=>{
 const {bindingsForPlatform}=await import('../src/data.js');
 const win=bindingsForPlatform('win32'),mac=bindingsForPlatform('darwin');
 assert.equal(win.find(b=>b[1]==='New session')[2],'⌃ T');
 assert.equal(win.find(b=>b[1]==='Copy selection')[2],'⌃ ⇧ C');
 assert.equal(win.find(b=>b[1]==='Paste')[2],'⌃ ⇧ V');
 assert.equal(mac.find(b=>b[1]==='New session')[2],'⌘ T');
});
