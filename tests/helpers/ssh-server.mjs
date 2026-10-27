import ssh2 from 'ssh2';
const {Server,utils}=ssh2;
export const serverKey=utils.generateKeyPairSync('ed25519');
export async function startSSHServer({password='test-only-password',publicKey,banner}={}){
 const stats={auth:0,resize:null};const clients=new Set();
 const server=new Server({hostKeys:[serverKey.private]},client=>{
  clients.add(client);client.on('error',()=>{});client.on('close',()=>clients.delete(client));
  client.on('authentication',ctx=>{stats.auth++;if(ctx.method==='password'&&ctx.password===password)ctx.accept();else if(ctx.method==='publickey'&&publicKey&&ctx.key.data.equals(publicKey.getPublicSSH())&&(!ctx.signature||publicKey.verify(ctx.blob,ctx.signature)))ctx.accept();else ctx.reject()});
  client.on('ready',()=>client.on('session',accept=>{const session=accept();session.on('pty',(accept,_reject,info)=>{stats.resize=info;accept()});session.on('window-change',(accept,_reject,info)=>{stats.resize=info;accept?.()});session.on('shell',accept=>{const stream=accept();stream.write(banner??'\x1b[32mHolonet integration ready\x1b[0m\r\n$ ');let input='';stream.on('data',chunk=>{input+=chunk.toString();while(/[\r\n]/.test(input)){const index=input.search(/[\r\n]/),cmd=input.slice(0,index);input=input.slice(index+1);if(!cmd)continue;if(cmd==='bulk'){stream.write('x'.repeat(512*1024));stream.write('\r\nBULK_DONE\r\n$ ');continue}if(cmd==='exit'){stream.exit(0);stream.end();return}stream.write(cmd+'\r\n'+(cmd==='whoami'?'integration-user':cmd==='pwd'?'/integration':cmd.startsWith('echo ')?cmd.slice(5):'received: '+cmd)+'\r\n$ ')}})})}));
 });
 await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(0,'127.0.0.1',resolve)});
 return {port:server.address().port,stats,close:async()=>{for(const client of clients)client.end();await new Promise(resolve=>server.close(resolve))}};
}
