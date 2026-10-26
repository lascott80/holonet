import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import os from 'node:os';import path from 'node:path';
import {randomBytes,createCipheriv,createDecipheriv} from 'node:crypto';
import ssh2 from 'ssh2';
import {CredentialVault,KnownHosts} from '../electron/secrets.mjs';
import {SSHSessions,passwordReference,fingerprint} from '../electron/ssh.mjs';
import {generateKey,publicKey} from '../electron/keys.mjs';
import {startSSHServer} from './helpers/ssh-server.mjs';
function encryption(){const key=randomBytes(32);return {available:async()=>true,encrypt:async text=>{const iv=randomBytes(12),cipher=createCipheriv('aes-256-gcm',key,iv);const body=Buffer.concat([cipher.update(text),cipher.final()]);return Buffer.concat([iv,cipher.getAuthTag(),body])},decrypt:async buffer=>{const decipher=createDecipheriv('aes-256-gcm',key,buffer.subarray(0,12));decipher.setAuthTag(buffer.subarray(12,28));return Buffer.concat([decipher.update(buffer.subarray(28)),decipher.final()]).toString()}}}
async function fixture(t){const directory=await mkdtemp(path.join(os.tmpdir(),'holonet-ssh-'));t.after(()=>rm(directory,{recursive:true,force:true}));const vault=new CredentialVault(directory,encryption()),knownHosts=new KnownHosts(directory);return {directory,vault,knownHosts}}
const host=port=>({id:'integration',name:'Integration',address:'127.0.0.1',port,user:'integration-user',auth:'Password'});
async function waitFor(check){const until=Date.now()+4000;while(Date.now()<until){if(check())return;await new Promise(resolve=>setTimeout(resolve,10))}assert.fail('Timed out waiting for SSH output')}
test('encrypted vault never writes plaintext and refuses an unavailable OS provider',async t=>{const {directory,vault}=await fixture(t);await vault.put('test','secret material');assert.equal(await vault.get('test'),'secret material');assert.doesNotMatch(await readFile(path.join(directory,'credentials.encrypted.json'),'utf8'),/secret material/);const unavailable=new CredentialVault(path.join(directory,'unavailable'),{available:async()=>false});await assert.rejects(unavailable.put('test','secret'),/unavailable/)});
test('real SSH verifies the host before password auth, opens PTY, writes output, resizes, and reconnects with saved password',async t=>{
 const f=await fixture(t),server=await startSSHServer();t.after(()=>server.close());const h=host(server.port),events=[];let verified=false;
 const manager=new SSHSessions({...f,emit:event=>{events.push(event);if(event.type==='host-key'){assert.equal(server.stats.auth,0);verified=true;manager.respond(event.requestId,true,true)}}});t.after(()=>manager.dispose());
 assert.deepEqual(await manager.connect(h),{needsPassword:true});
 await manager.connect(h,{password:'test-only-password',remember:true});assert.equal(verified,true);assert.equal(await f.vault.get(passwordReference(h)),'test-only-password');
 manager.write(h.id,'whoami\r');await waitFor(()=>Buffer.from(manager.snapshot(h.id).data,'base64').toString().includes('integration-user'));manager.resize(h.id,123,42);await waitFor(()=>server.stats.resize?.cols===123);assert.equal(server.stats.resize.rows,42);
 const count=events.filter(e=>e.type==='host-key').length;manager.disconnect(h.id);await manager.connect(h);assert.equal(events.filter(e=>e.type==='host-key').length,count);
});
test('rejecting or changing a host key prevents authentication',async t=>{
 const f=await fixture(t),server=await startSSHServer();t.after(()=>server.close());const h=host(server.port);
 let manager=new SSHSessions({...f,emit:event=>{if(event.type==='host-key')manager.respond(event.requestId,false,false)}});t.after(()=>manager.dispose());
 await assert.rejects(manager.connect(h,{password:'test-only-password'}));assert.equal(server.stats.auth,0);
 await f.knownHosts.remember(h,{fingerprint:'SHA256:another-server',algorithm:'ssh-ed25519'});const events=[];manager=new SSHSessions({...f,emit:event=>events.push(event)});await assert.rejects(manager.connect(h,{password:'test-only-password'}));assert.equal(server.stats.auth,0);assert.ok(events.some(e=>e.type==='host-key-changed'));assert.equal((await f.knownHosts.get(h)).fingerprint,'SHA256:another-server');
});
test('failed password auth is not remembered',async t=>{const f=await fixture(t),server=await startSSHServer();t.after(()=>server.close());const h=host(server.port);const manager=new SSHSessions({...f,emit:event=>{if(event.type==='host-key')manager.respond(event.requestId,true,false)}});t.after(()=>manager.dispose());await assert.rejects(manager.connect(h,{password:'wrong-password',remember:true}));assert.equal(await f.vault.get(passwordReference(h)),undefined)});
test('encrypted generated keys need a passphrase unless explicitly remembered, and authenticate over SSH',async t=>{
 const f=await fixture(t);const metadata=await generateKey(f.vault,{name:'Test key',passphrase:'test-passphrase'});assert.match(metadata.fingerprint,/^SHA256:/);assert.match(await publicKey(f.vault,metadata.id),/^ssh-ed25519 /);
 const key=JSON.parse(await f.vault.get('key:'+metadata.id));assert.equal(key.passphrase,'');const parsed=ssh2.utils.parseKey(key.privateKey,'test-passphrase');
 const server=await startSSHServer({publicKey:parsed});t.after(()=>server.close());const h={...host(server.port),auth:'SSH key',key:metadata.id};const manager=new SSHSessions({...f,emit:event=>{if(event.type==='host-key')manager.respond(event.requestId,true,false)}});t.after(()=>manager.dispose());
 assert.deepEqual(await manager.connect(h),{needsPassphrase:true});await assert.rejects(manager.connect(h,{keyPassphrase:'wrong'}),/incorrect/);await manager.connect(h,{keyPassphrase:'test-passphrase',remember:false});assert.equal(manager.snapshot(h.id).state,'connected');assert.equal(JSON.parse(await f.vault.get('key:'+metadata.id)).passphrase,'');
});
test('password references change when connection identity changes',()=>{const h=host(22);assert.notEqual(passwordReference(h),passwordReference({...h,address:'other.test'}));assert.notEqual(passwordReference(h),passwordReference({...h,user:'other'}));assert.notEqual(passwordReference(h),passwordReference({...h,port:2222}))});

test('terminal output pauses until the renderer acknowledges buffered bytes',async t=>{const f=await fixture(t),server=await startSSHServer();t.after(()=>server.close());const h=host(server.port);const manager=new SSHSessions({...f,emit:event=>{if(event.type==='host-key')manager.respond(event.requestId,true,false)}});t.after(()=>manager.dispose());await manager.connect(h,{password:'test-only-password'});manager.write(h.id,'bulk\r');await waitFor(()=>manager.snapshot(h.id).pendingBytes>128*1024);assert.equal(manager.sessions.get(h.id).channel.isPaused(),true);await waitFor(()=>{const snapshot=manager.snapshot(h.id);manager.acknowledge(h.id,snapshot.pendingBytes);return Buffer.from(snapshot.data,'base64').toString().includes('BULK_DONE')});});
test('typing exit reports one intentional shell exit for the tab-disconnect policy',async t=>{
 const f=await fixture(t),server=await startSSHServer();t.after(()=>server.close());const h=host(server.port),events=[];
 const manager=new SSHSessions({...f,emit:event=>{events.push(event);if(event.type==='host-key')manager.respond(event.requestId,true,false)}});t.after(()=>manager.dispose());
 await manager.connect(h,{password:'test-only-password'});manager.write(h.id,'exit\r');
 await waitFor(()=>events.some(e=>e.type==='state'&&e.state==='disconnected'));
 assert.equal(events.filter(e=>e.type==='state'&&e.state==='disconnected').length,1);
 assert.equal(events.find(e=>e.type==='state'&&e.state==='disconnected').reason,'shell-exit');
});
