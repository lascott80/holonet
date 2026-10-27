import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs/promises';import os from 'node:os';import path from 'node:path';import net from 'node:net';import {once} from 'node:events';
import {effectiveHost,accelerator} from '../shared/model.mjs';import {sanitizeState} from '../shared/state.mjs';import {initialGroups,initialSettings,defaultBindings} from '../src/data.js';import {SSHSessions} from '../electron/ssh.mjs';import {KnownHosts} from '../electron/secrets.mjs';import {DesktopTools} from '../electron/tools.mjs';import {toolsServer} from './helpers/tools-server.mjs';
const wait=async check=>{for(let i=0;i<300;i++){if(check())return;await new Promise(r=>setTimeout(r,15))}throw Error('Timed out')};
test('group inheritance ignores empty defaults and applies explicit overrides / reusable identity',()=>{const groups=[{id:'root',name:'Lab',defaults:{user:'root',port:'2222',identity:'shared'}},{id:'child',name:'Servers',parent:'Lab',parentId:'root',defaults:{user:'',port:''}}],identity={id:'shared',name:'Shared',user:'deploy',auth:'SSH key',key:'key1'};const h=effectiveHost({id:'h',groupId:'child',user:'',port:''},groups,[identity]);assert.equal(h.port,2222);assert.equal(h.user,'root');assert.equal(h.key,'key1');assert.equal(effectiveHost({...h,user:'override',port:2200},groups,[identity]).port,2200);assert.equal(accelerator('⌃ ⇧ V'),'Control+Shift+V')});
test('new metadata survives sanitization without exposing secrets; distinct session IDs are preserved',()=>{const state=sanitizeState({version:1,hosts:[{id:'h',name:'Host',address:'example.test',user:'',port:'',groupId:'Lab',password:'secret'}],groups:initialGroups,keyMetadata:[],settings:initialSettings,bindings:defaultBindings,identities:[{id:'i',name:'Identity',user:'u',auth:'Password',password:'secret'}],snippets:[{id:'s',name:'Script',script:'pwd'}],forwardings:[],sessionTabs:[{id:'one',host:'h',mode:'terminal'},{id:'two',host:'h',mode:'terminal'}]});assert.equal(state.sessionTabs.length,2);assert.doesNotMatch(JSON.stringify(state),/secret|password/);assert.equal(state.hosts[0].port,'')});
test('a saved identity supplies username and key to a new host without explicit credentials',()=>{
 const identity={id:'shared',name:'Personal',user:'lascott',auth:'SSH key',key:'key1'};
 const resolved=effectiveHost({id:'new',address:'example.test',user:'',identity:'shared'},[],[identity]);
 assert.equal(resolved.user,'lascott');assert.equal(resolved.auth,'SSH key');assert.equal(resolved.key,'key1');
 assert.equal(effectiveHost({...resolved,user:'override'},[],[identity]).user,'override');
});
async function fixture(t){const dir=await fs.mkdtemp(path.join(os.tmpdir(),'holonet-tools-'));const remote=path.join(dir,'remote');await fs.mkdir(remote);const server=await toolsServer(remote),events=[],knownHosts=new KnownHosts(dir);const secrets=new Map(),vault={get:async id=>secrets.get(id),put:async(id,value)=>secrets.set(id,value)};const ssh=new SSHSessions({vault,knownHosts,emit:e=>{events.push(e);if(e.type==='host-key')ssh.respond(e.requestId,true,true)}});const tools=new DesktopTools({ssh,knownHosts,directory:dir,emit:e=>events.push(e)});t.after(async()=>{tools.dispose();ssh.dispose();await server.close();await fs.rm(dir,{recursive:true,force:true})});const host={id:'saved',name:'Fixture',address:'127.0.0.1',port:server.port,user:'tester',auth:'Password'};await ssh.connect(host,{password:'fixture',sessionId:'one',remember:true});return {dir,remote,events,ssh,tools,host}}
test('independent live sessions for a saved host reuse its encrypted-vault credential reference',async t=>{const f=await fixture(t);await f.ssh.connect(f.host,{sessionId:'two'});assert.equal(f.ssh.sessions.size,2);f.ssh.disconnect('one');assert.equal(f.ssh.sessions.get('two').state,'connected')});
test('real SFTP lists, uploads/downloads, prompts on conflict and applies rename / permissions',async t=>{const f=await fixture(t),source=path.join(f.dir,'source.txt'),dest=path.join(f.dir,'download.txt');await fs.writeFile(source,'transfer fixture\n'.repeat(10000));const listing=await f.tools.list({side:'remote',sessionId:'one',path:'.'});assert.equal(listing.path,'/');await f.tools.fileAction({side:'remote',sessionId:'one',operation:'mkdir',path:'/folder'});const result=await f.tools.transfer({sessionId:'one',direction:'upload',local:source,remote:'/folder/upload.txt',replace:false});await wait(()=>f.events.some(e=>e.id===result.id&&e.state==='completed'));assert.equal(await fs.readFile(path.join(f.remote,'folder/upload.txt'),'utf8'),await fs.readFile(source,'utf8'));assert.deepEqual(await f.tools.transfer({sessionId:'one',direction:'upload',local:source,remote:'/folder/upload.txt',replace:false}),{conflict:true});await f.tools.fileAction({side:'remote',sessionId:'one',operation:'rename',path:'/folder/upload.txt',destination:'/folder/renamed.txt'});await f.tools.fileAction({side:'remote',sessionId:'one',operation:'chmod',path:'/folder/renamed.txt',mode:'640'});const download=await f.tools.transfer({sessionId:'one',direction:'download',local:dest,remote:'/folder/renamed.txt',replace:false});await wait(()=>f.events.some(e=>e.id===download.id&&e.state==='completed'));assert.equal(await fs.readFile(dest,'utf8'),await fs.readFile(source,'utf8'));assert.equal((await fs.stat(path.join(f.remote,'folder/renamed.txt'))).mode&0o777,0o640);assert.deepEqual((await fs.readdir(f.remote)).sort(),['folder'])});
async function listen(server){await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve)});return server.address().port}
async function freePort(){const s=net.createServer();const port=await listen(s);await new Promise(r=>s.close(r));return port}
async function echoAt(port){const socket=net.connect(port,'127.0.0.1');socket.setTimeout(3000,()=>socket.destroy(Error('Timeout')));await once(socket,'connect');socket.write('through SSH');const [body]=await once(socket,'data');socket.destroy();assert.equal(body.toString(),'through SSH')}
test('local and remote TCP forwarding carry real bytes and stop their listeners',async t=>{const f=await fixture(t),echo=net.createServer(s=>s.pipe(s)),destinationPort=await listen(echo);t.after(()=>echo.close());for(const type of ['local','remote']){const port=await freePort(),rule={id:type,type,bind:'127.0.0.1',port,destination:'127.0.0.1',destinationPort};await f.tools.startForward(rule,'one');await echoAt(port);await f.tools.stopForward(type);assert.equal(f.tools.tunnels.has(type),false)}});
test('dynamic SOCKS5 negotiates a domain target and carries bytes through SSH',async t=>{const f=await fixture(t),echo=net.createServer(s=>s.pipe(s)),destinationPort=await listen(echo);t.after(()=>echo.close());const port=await freePort();await f.tools.startForward({id:'socks',type:'dynamic',bind:'127.0.0.1',port},'one');const socket=net.connect(port,'127.0.0.1');t.after(()=>socket.destroy());socket.setTimeout(3000,()=>socket.destroy(Error('Timeout')));await once(socket,'connect');socket.write(Buffer.from([5,1,0]));const [greeting]=await once(socket,'data');assert.deepEqual([...greeting],[5,0]);const host=Buffer.from('127.0.0.1'),request=Buffer.alloc(7+host.length);request.set([5,1,0,3,host.length]);host.copy(request,5);request.writeUInt16BE(destinationPort,5+host.length);socket.write(request);const [reply]=await once(socket,'data');assert.equal(reply[1],0);socket.write('SOCKS echo');const [body]=await once(socket,'data');assert.equal(body.toString(),'SOCKS echo');socket.destroy();await f.tools.stopForward('socks')});
test('cancelling an active transfer removes partial files and leaves destination untouched',async t=>{const f=await fixture(t),source=path.join(f.dir,'large.bin');await fs.writeFile(source,Buffer.alloc(8*1024*1024,7));const result=await f.tools.transfer({sessionId:'one',direction:'upload',local:source,remote:'/cancelled.bin',replace:false});f.tools.cancel(result.id);await wait(()=>f.events.some(e=>e.id===result.id&&e.state==='cancelled'));await wait(()=>!f.tools.transfers.has(result.id));assert.deepEqual(await fs.readdir(f.remote),[])});
test('an occupied forwarding port fails without recording a running rule',async t=>{const f=await fixture(t),occupied=net.createServer();const port=await listen(occupied);t.after(()=>occupied.close());await assert.rejects(f.tools.startForward({id:'occupied',type:'local',bind:'127.0.0.1',port,destination:'127.0.0.1',destinationPort:80},'one'),/EADDRINUSE/);assert.equal(f.tools.tunnels.has('occupied'),false)});
test('quick connection parsing supports SSH syntax, ports and IPv6 without interpreting shell commands',async()=>{const {parseConnection}=await import('../shared/model.mjs');assert.equal(parseConnection('ssh user@example.test -p 2222').port,2222);assert.equal(parseConnection('ssh root@[::1]:2200').address,'::1');assert.equal(parseConnection('search text'),null);assert.equal(parseConnection('ssh host; rm -rf /'),null);assert.equal(parseConnection('ssh user@host:70000'),null)});

test('SFTP deletion removes files and empty folders without recursively deleting contents',async t=>{
 const f=await fixture(t);
 for(const side of ['local','remote']){
  const base=side==='local'?f.dir:'/';
  const target=name=>side==='local'?path.join(base,name):'/'+name;
  const disk=name=>side==='local'?path.join(f.dir,name):path.join(f.remote,name);
  await fs.writeFile(disk('delete-me'),'test');
  await f.tools.fileAction({side,sessionId:'one',operation:'delete',path:target('delete-me')});
  await assert.rejects(fs.stat(disk('delete-me')), {code:'ENOENT'});
  await fs.mkdir(disk('empty'));
  await f.tools.fileAction({side,sessionId:'one',operation:'delete',path:target('empty')});
  await assert.rejects(fs.stat(disk('empty')), {code:'ENOENT'});
  await fs.mkdir(disk('occupied'));await fs.writeFile(path.join(disk('occupied'),'keep'),'safe');
  await assert.rejects(f.tools.fileAction({side,sessionId:'one',operation:'delete',path:target('occupied')}));
  assert.equal(await fs.readFile(path.join(disk('occupied'),'keep'),'utf8'),'safe');
 }
});

test('permission changes support special mode bits on local and remote files',async t=>{
 const f=await fixture(t);
 for(const side of ['local','remote']){
  const disk=path.join(side==='local'?f.dir:f.remote,'permissions-test');
  await fs.writeFile(disk,'test');
  await f.tools.fileAction({side,sessionId:'one',operation:'chmod',path:side==='local'?disk:'/permissions-test',mode:'1744'});
  assert.equal((await fs.stat(disk)).mode&0o7777,0o1744);
 }
});
test('clearing connection history persists and does not recreate records for existing sessions',async t=>{
 const f=await fixture(t),session=f.ssh.sessions.get('one');
 await f.tools.record({type:'state',host:'one',state:'connected'});
 assert.equal(Object.keys((await f.tools.history.read()).entries).length,1);
 const queued=f.tools.record({type:'state',host:'one',state:'connected'});
 await Promise.all([queued,f.tools.clearHistory()]);
 await f.tools.record({type:'state',host:'one',state:'disconnected'});
 assert.deepEqual((await f.tools.history.read()).entries,{});
 const reloaded=new DesktopTools({ssh:f.ssh,knownHosts:f.knownHosts,directory:f.dir,emit:()=>{}});
 assert.deepEqual((await reloaded.history.read()).entries,{});
 const fresh={...session,host:{...session.host},historyCleared:false};delete fresh.logId;
 f.ssh.sessions.set('new-session',fresh);
 await reloaded.record({type:'state',host:'new-session',state:'connected'});
 assert.equal(Object.keys((await reloaded.history.read()).entries).length,1);
 f.ssh.sessions.delete('new-session');
});
