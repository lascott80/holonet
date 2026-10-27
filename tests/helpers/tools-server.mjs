import ssh2 from 'ssh2';
import fs from 'node:fs/promises';
import path from 'node:path';
import net from 'node:net';
const {Server,utils}=ssh2;
export async function toolsServer(root){
 const hostKey=utils.generateKeyPairSync('ed25519'),clients=new Set(),listeners=new Set();
 const attrs=s=>({mode:s.mode,size:s.size,uid:s.uid,gid:s.gid,atime:Math.floor(s.atimeMs/1000),mtime:Math.floor(s.mtimeMs/1000)});
 const local=p=>{const value=path.resolve(root,'.'+(p.startsWith('/')?p:'/'+p));if(value!==root&&!value.startsWith(root+path.sep))throw Error('Outside test root');return value};
 const server=new Server({hostKeys:[hostKey.private]},client=>{clients.add(client);client.on('error',()=>{});client.on('close',()=>clients.delete(client));client.on('authentication',ctx=>ctx.method==='password'&&ctx.password==='fixture'?ctx.accept():ctx.reject());
 client.on('ready',()=>{
 client.on('tcpip',(accept,reject,info)=>{const socket=net.connect(info.destPort,info.destIP);socket.on('error',()=>reject());socket.on('connect',()=>{const stream=accept();socket.pipe(stream).pipe(socket);stream.on('error',()=>socket.destroy());stream.on('close',()=>socket.destroy())})});
 client.on('request',(accept,reject,name,info)=>{if(name==='tcpip-forward'){const listener=net.createServer(socket=>{client.forwardOut(info.bindAddr,info.bindPort,socket.remoteAddress,socket.remotePort,(err,stream)=>{if(err){socket.destroy();return}socket.pipe(stream).pipe(socket);stream.on('error',()=>socket.destroy());socket.on('error',()=>stream.destroy())})});listener.on('error',()=>reject());listener.listen(info.bindPort,info.bindAddr,()=>{listeners.add(listener);listener.rule=info;accept(listener.address().port)})}else if(name==='cancel-tcpip-forward'){for(const listener of listeners)if(listener.rule.bindPort===info.bindPort){listener.close();listeners.delete(listener)}accept()}});
 client.on('session',accept=>{const session=accept();session.on('pty',accept=>accept());session.on('shell',accept=>{const stream=accept();stream.write('Fixture shell\r\n$ ');stream.on('data',data=>stream.write(data))});session.on('sftp',accept=>{
 const sftp=accept(),handles=new Map();let count=0;const status=(id,e)=>sftp.status(id,e?(e.code==='ENOENT'?2:4):0,e?.message);
 const on=(name,fn)=>sftp.on(name,(id,...args)=>Promise.resolve().then(()=>fn(id,...args)).catch(e=>status(id,e)));
 const handle=value=>{const b=Buffer.alloc(4);b.writeUInt32BE(++count);handles.set(count,value);return b};const get=b=>handles.get(b.readUInt32BE());
 on('REALPATH',async(id,p)=>{local(p);sftp.name(id,[{filename:p==='.'?'/':p,longname:p,attrs:{}}])});
 on('STAT',async(id,p)=>sftp.attrs(id,attrs(await fs.stat(local(p)))));on('LSTAT',async(id,p)=>sftp.attrs(id,attrs(await fs.lstat(local(p)))));
 on('OPENDIR',async(id,p)=>{const names=await fs.readdir(local(p));sftp.handle(id,handle({dir:p,names,read:false}))});
 on('READDIR',async(id,b)=>{const h=get(b);if(h.read){sftp.status(id,1);return}h.read=true;const entries=await Promise.all(h.names.map(async name=>({filename:name,longname:name,attrs:attrs(await fs.stat(local(path.posix.join(h.dir,name))))})));if(entries.length)sftp.name(id,entries);else sftp.status(id,1)});
 on('OPEN',async(id,p,flags,a)=>{const file=await fs.open(local(p),utils.sftp.flagsToString(flags),a.mode);sftp.handle(id,handle({file}))});
 on('FSTAT',async(id,b)=>sftp.attrs(id,attrs(await get(b).file.stat())));
 on('READ',async(id,b,offset,length)=>{const buf=Buffer.alloc(length),{bytesRead}=await get(b).file.read(buf,0,length,offset);if(bytesRead)sftp.data(id,buf.subarray(0,bytesRead));else sftp.status(id,1)});
 on('WRITE',async(id,b,offset,data)=>{await get(b).file.write(data,0,data.length,offset);status(id)});
 on('CLOSE',async(id,b)=>{await get(b)?.file?.close();handles.delete(b.readUInt32BE());status(id)});
 on('MKDIR',async(id,p)=>{await fs.mkdir(local(p));status(id)});on('RMDIR',async(id,p)=>{await fs.rmdir(local(p));status(id)});on('RENAME',async(id,a,b)=>{await fs.rename(local(a),local(b));status(id)});on('REMOVE',async(id,p)=>{await fs.unlink(local(p));status(id)});on('SETSTAT',async(id,p,a)=>{if(a.mode!=null)await fs.chmod(local(p),a.mode);status(id)});
 sftp.on('close',()=>{for(const h of handles.values())h.file?.close().catch(()=>{})});
 })});
 });
 });
 await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve)});
 return {port:server.address().port,close:async()=>{for(const l of listeners)l.close();for(const c of clients)c.end();await new Promise(r=>server.close(r))}};
}
