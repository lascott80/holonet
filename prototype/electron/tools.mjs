import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import {pipeline} from 'node:stream/promises';
import {randomUUID} from 'node:crypto';
import {PrivateDocument} from './secrets.mjs';
const call=(obj,method,...args)=>new Promise((resolve,reject)=>obj[method](...args,(err,...values)=>err?reject(err):resolve(values.length>1?values:values[0])));
function safePath(value){if(typeof value!=='string'||!value||value.length>8192||value.includes('\0'))throw Error('Invalid path');return value}
export class DesktopTools {
 constructor({ssh,knownHosts,directory,emit}){this.ssh=ssh;this.knownHosts=knownHosts;this.emit=emit;this.transfers=new Map();this.tunnels=new Map();this.history=new PrivateDocument(path.join(directory,'connections.json'),{version:1,entries:{}})}
 async record(event){if(event.type!=='state')return;const s=this.ssh.sessions.get(event.host);if(!s)return;await this.history.update(data=>{const logId=s.logId||(s.logId=randomUUID());data.entries[logId]={id:logId,name:s.host.name,address:s.host.address,user:s.host.user,started:data.entries[logId]?.started||Date.now(),state:event.state,message:event.message,updated:Date.now()};const all=Object.values(data.entries).sort((a,b)=>b.updated-a.updated);data.entries=Object.fromEntries(all.slice(0,500).map(x=>[x.id,x]))})}
 session(id){const s=this.ssh.sessions.get(id);if(s?.state!=='connected')throw Error('Connect an SSH session first.');return s}
 async sftp(id){const s=this.session(id);if(!s.sftpPromise)s.sftpPromise=call(s.client,'sftp').catch(e=>{s.sftpPromise=null;throw e});return s.sftpPromise}
 local(value){value=safePath(value);return value==='~'?os.homedir():value.startsWith('~/')?path.join(os.homedir(),value.slice(2)):path.resolve(value)}
 async list({side,sessionId,path:input}){
  if(side==='local'){const dir=this.local(input||'~'),entries=await fsp.readdir(dir,{withFileTypes:true});const items=await Promise.all(entries.slice(0,10000).map(async item=>{let st;try{st=await fsp.stat(path.join(dir,item.name))}catch{}return {name:item.name,directory:st?.isDirectory()||item.isDirectory(),size:st?.size||0,modified:st?.mtimeMs||0,mode:st?.mode||0}}));return {path:dir,parent:path.dirname(dir),items}}
  const sftp=await this.sftp(sessionId),dir=await call(sftp,'realpath',safePath(input||'.')),items=await call(sftp,'readdir',dir);
  return {path:dir,parent:path.posix.dirname(dir),items:items.map(i=>({name:i.filename,directory:i.attrs.isDirectory(),size:i.attrs.size,modified:i.attrs.mtime*1000,mode:i.attrs.mode}))};
 }
 async fileAction(r){const target=safePath(r.path),remote=r.side==='remote',sftp=remote?await this.sftp(r.sessionId):null;
  if(r.operation==='mkdir')return remote?call(sftp,'mkdir',target):fsp.mkdir(this.local(target));
  if(r.operation==='rename'){safePath(r.destination);return remote?call(sftp,'rename',target,r.destination):fsp.rename(this.local(target),this.local(r.destination))}
  if(r.operation==='chmod'){if(!/^[0-7]{3,4}$/.test(r.mode))throw Error('Use octal permissions, for example 644');return remote?call(sftp,'chmod',target,parseInt(r.mode,8)):fsp.chmod(this.local(target),parseInt(r.mode,8))}
  throw Error('Unsupported file action');
 }
 async transfer(r){
  const sftp=await this.sftp(r.sessionId),local=this.local(r.local),remote=safePath(r.remote);let size;
  if(r.direction==='upload')size=(await fsp.stat(local)).size;else size=(await call(sftp,'stat',remote)).size;
  const exists=await (r.direction==='upload'?call(sftp,'stat',remote):fsp.stat(local)).then(()=>true,e=>{if(e.code===2||e.code==='ENOENT')return false;throw e});
  if(exists&&!r.replace)return {conflict:true};
  const id=randomUUID(),partial=r.direction==='upload'?remote+'.holonet-'+id:local+'.holonet-'+id;
  const entry={id,name:path.basename(local),direction:r.direction,state:'queued',bytes:0,total:size,sessionId:r.sessionId};this.transfers.set(id,entry);
  let last=0;const send=()=>this.emit({type:'transfer',id,sessionId:r.sessionId,name:entry.name,direction:entry.direction,state:entry.state,bytes:entry.bytes,total:size,message:entry.message});
  entry.run=()=>{
   entry.state='active';
   const source=r.direction==='upload'?fs.createReadStream(local):sftp.createReadStream(remote);
   const dest=r.direction==='upload'?sftp.createWriteStream(partial,{flags:'wx',mode:0o600}):fs.createWriteStream(partial,{flags:'wx',mode:0o600});
   entry.source=source;entry.dest=dest;source.on('data',chunk=>{entry.bytes+=chunk.length;if(Date.now()-last>100){last=Date.now();send()}});send();
   pipeline(source,dest).then(async()=>{if(entry.state==='cancelled')throw Error('Cancelled');if(r.direction==='upload'){
     if(r.replace&&typeof sftp.ext_openssh_rename==='function'){
      try{await call(sftp,'ext_openssh_rename',partial,remote)}catch(e){if(e.code!==8)throw e;await call(sftp,'rename',partial,remote)}
     }else await call(sftp,'rename',partial,remote);
    }else if(r.replace)await fsp.rename(partial,local);else{await fsp.link(partial,local);await fsp.unlink(partial)}entry.state='completed'}).catch(async e=>{if(entry.state!=='cancelled'){entry.state='failed';entry.message=e.message}try{if(r.direction==='upload')await call(sftp,'unlink',partial);else await fsp.unlink(partial)}catch{}}).finally(()=>{send();this.transfers.delete(id);this.drainTransfers()});
  };
  send();this.drainTransfers();
  return {id};
 }
 drainTransfers(){let active=[...this.transfers.values()].filter(t=>t.state==='active').length;for(const t of this.transfers.values())if(active<2&&t.state==='queued'){active++;try{this.session(t.sessionId);t.run()}catch(e){t.state='failed';t.message=e.message;this.emit({type:'transfer',id:t.id,sessionId:t.sessionId,name:t.name,direction:t.direction,state:t.state,bytes:0,total:t.total,message:t.message});this.transfers.delete(t.id);active--}}}
 cancel(id){const t=this.transfers.get(id);if(t){t.state='cancelled';if(t.source){t.source.destroy(Error('Cancelled'));t.dest.destroy(Error('Cancelled'))}else{this.emit({type:'transfer',id:t.id,sessionId:t.sessionId,name:t.name,direction:t.direction,state:'cancelled',bytes:0,total:t.total});this.transfers.delete(id)}}}

 async startForward(rule,sessionId){
  if(this.tunnels.has(rule.id))return {running:true};const s=this.session(sessionId),sockets=new Set();
  const entry={rule,sessionId,sockets};const pipe=(socket,host,port,onReady=()=>{})=>{sockets.add(socket);socket.on('error',()=>{});socket.on('close',()=>sockets.delete(socket));s.client.forwardOut(socket.remoteAddress||'127.0.0.1',socket.remotePort||0,host,port,(error,stream)=>{if(error){socket.destroy();this.emit({type:'forward-error',id:rule.id,message:error.message});return}if(socket.destroyed){stream.destroy();return}onReady(socket);socket.pipe(stream).pipe(socket);stream.on('error',()=>socket.destroy());socket.on('close',()=>stream.destroy())})};
  if(rule.type==='remote'){
   await call(s.client,'forwardIn',rule.bind,rule.port);
   entry.listener=(info,accept,reject)=>{if(info.destPort!==rule.port||info.destIP!==rule.bind)return;const socket=net.connect(rule.destinationPort,rule.destination);sockets.add(socket);socket.on('error',()=>{reject();sockets.delete(socket)});socket.once('connect',()=>{const stream=accept();socket.pipe(stream).pipe(socket);stream.on('error',()=>socket.destroy());socket.on('close',()=>{sockets.delete(socket);stream.destroy()})})};s.client.on('tcp connection',entry.listener);
  }else{
   entry.server=net.createServer(socket=>{if(rule.type==='local')pipe(socket,rule.destination,rule.destinationPort);else this.socks(socket,pipe,sockets)});
   await new Promise((resolve,reject)=>{entry.server.once('error',reject);entry.server.listen(rule.port,rule.bind,resolve)});
   entry.server.on('error',e=>this.emit({type:'forward-error',id:rule.id,message:e.message}));
  }
  this.tunnels.set(rule.id,entry);entry.closed=()=>this.stopForward(rule.id);s.client.once('close',entry.closed);this.emit({type:'forward',id:rule.id,running:true});return {running:true};
 }
 socks(socket,pipe,sockets){
  sockets.add(socket);socket.on('error',()=>{});socket.once('close',()=>sockets.delete(socket));socket.setTimeout(15000,()=>socket.destroy());let buffer=Buffer.alloc(0),stage=0;
  const data=chunk=>{buffer=Buffer.concat([buffer,chunk]);if(buffer.length>65536){socket.destroy();return}if(stage===0){if(buffer.length<2)return;if(buffer[0]!==5){socket.destroy();return}const n=buffer[1];if(buffer.length<n+2)return;if(!buffer.subarray(2,n+2).includes(0)){socket.end(Buffer.from([5,255]));return}buffer=buffer.subarray(n+2);socket.write(Buffer.from([5,0]));stage=1}
   if(stage===1){if(buffer.length<4)return;if(buffer[0]!==5||buffer[1]!==1){socket.end(Buffer.from([5,7,0,1,0,0,0,0,0,0]));return}let length,host,offset;if(buffer[3]===1){length=10;offset=8;if(buffer.length<length)return;host=[...buffer.subarray(4,8)].join('.')}else if(buffer[3]===3){if(buffer.length<5)return;length=7+buffer[4];offset=5+buffer[4];if(buffer.length<length)return;host=buffer.subarray(5,offset).toString()}else if(buffer[3]===4){length=22;offset=20;if(buffer.length<length)return;host=Array.from({length:8},(_,i)=>buffer.readUInt16BE(4+i*2).toString(16)).join(':')}else{socket.destroy();return}const port=buffer.readUInt16BE(offset),rest=buffer.subarray(length);socket.pause();socket.removeListener('data',data);if(rest.length)socket.unshift(rest);socket.setTimeout(0);pipe(socket,host,port,()=>{socket.write(Buffer.from([5,0,0,1,0,0,0,0,0,0]));socket.resume()});stage=2}
  };socket.on('data',data);
 }
 async stopForward(id){const t=this.tunnels.get(id);if(!t)return;this.tunnels.delete(id);for(const socket of t.sockets)socket.destroy();t.server?.close();const s=this.ssh.sessions.get(t.sessionId);if(s){s.client.removeListener('close',t.closed);if(t.listener){s.client.removeListener('tcp connection',t.listener);await call(s.client,'unforwardIn',t.rule.bind,t.rule.port).catch(()=>{})}}this.emit({type:'forward',id,running:false})}
 dispose(){for(const id of this.transfers.keys())this.cancel(id);for(const id of this.tunnels.keys())this.stopForward(id).catch(()=>{})}
}
