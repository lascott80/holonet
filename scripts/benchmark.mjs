// Repeatable local-only baseline. This never reads the user's Holonet profile.
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {performance} from 'node:perf_hooks';
import {DesktopTools} from '../electron/tools.mjs';
import {SSHSessions} from '../electron/ssh.mjs';
import {KnownHosts} from '../electron/secrets.mjs';
import {startSSHServer} from '../tests/helpers/ssh-server.mjs';
import {toolsServer} from '../tests/helpers/tools-server.mjs';
import {mkdir} from 'node:fs/promises';
const dir=await mkdtemp(path.join(os.tmpdir(),'holonet-bench-'));
let server,ssh,tools,sftpServer,sftpSSH,sftpTools;
const measure=async(name,fn)=>{const start=performance.now();await fn();console.log(`${name}: ${(performance.now()-start).toFixed(1)} ms`)};
try{
 await measure('Create 1000 fixture files',()=>Promise.all(Array.from({length:1000},(_,i)=>writeFile(path.join(dir,`file-${i}`),''))));
 server=await startSSHServer();const knownHosts=new KnownHosts(dir),vault={get:async()=>undefined,put:async()=>{}};
 let bulkResolve;const bulkDone=new Promise(resolve=>{bulkResolve=resolve});ssh=new SSHSessions({vault,knownHosts,emit:event=>{if(event.type==='host-key')ssh.respond(event.requestId,true,false);if(event.type==='data'){const data=Buffer.from(event.data,'base64');ssh.acknowledge(event.host,data.length);if(data.includes('BULK_DONE'))bulkResolve()}}});
 tools=new DesktopTools({ssh,knownHosts,directory:dir,emit:()=>{}});
 await measure('Local 1000-entry listing',()=>tools.list({side:'local',path:dir}));
 const host={id:'benchmark',name:'Loopback',address:'127.0.0.1',port:server.port,user:'integration-user',auth:'Password'};
 await measure('Loopback SSH connection',()=>ssh.connect(host,{password:'test-only-password'}));
 await measure('Open nine additional SSH sessions',()=>Promise.all(Array.from({length:9},(_,i)=>ssh.connect(host,{sessionId:`bench-${i}`,password:'test-only-password'}))));
 await measure('512 KiB terminal output',async()=>{ssh.write('benchmark','bulk\r');await bulkDone});
 const remote=path.join(dir,'remote');await mkdir(remote);sftpServer=await toolsServer(remote);sftpSSH=new SSHSessions({vault,knownHosts,emit:event=>{if(event.type==='host-key')sftpSSH.respond(event.requestId,true,false)}});let transferResolve;const transferDone=new Promise(resolve=>{transferResolve=resolve});sftpTools=new DesktopTools({ssh:sftpSSH,knownHosts,directory:dir,emit:event=>{if(event.type==='transfer'&&['completed','failed','cancelled'].includes(event.state))transferResolve(event)}});await sftpSSH.connect({id:'transfer',name:'Transfer',address:'127.0.0.1',port:sftpServer.port,user:'tester',auth:'Password'},{password:'fixture'});const payload=path.join(dir,'payload');await writeFile(payload,Buffer.alloc(1024*1024));await measure('1 MiB SFTP upload',async()=>{await sftpTools.transfer({sessionId:'transfer',direction:'upload',local:payload,remote:'/payload',replace:false});const event=await transferDone;if(event.state!=='completed')throw Error(event.message||event.state)});
 console.log(`Node RSS with 10 sessions: ${(process.memoryUsage().rss/1024/1024).toFixed(1)} MiB`);
 console.log('For renderer startup and renderer memory, use a disposable desktop profile and record hardware and build version.');
}finally{sftpTools?.dispose();sftpSSH?.dispose();if(sftpServer)await sftpServer.close();tools?.dispose();ssh?.dispose();if(server)await server.close();await rm(dir,{recursive:true,force:true})}
