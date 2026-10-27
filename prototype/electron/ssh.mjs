import ssh2 from 'ssh2';
const {Client,utils}=ssh2;
import {createHash,randomUUID} from 'node:crypto';
const fingerprint=key=>'SHA256:'+createHash('sha256').update(key).digest('base64').replace(/=+$/,'');
export const passwordReference=host=>host.credentialIdentity?'identity:'+host.credentialIdentity+':'+createHash('sha256').update(host.user).digest('hex'):'host:'+(host.credentialHostId||host.id)+':'+createHash('sha256').update(JSON.stringify([host.address.toLowerCase(),Number(host.port||22),host.user])).digest('hex');
export class SSHSessions {
  constructor({vault,knownHosts,emit,clientFactory=()=>new Client()}){this.vault=vault;this.knownHosts=knownHosts;this.emit=emit;this.clientFactory=clientFactory;this.sessions=new Map();this.verifications=new Map()}
  state(id,state,message='',reason){const session=this.sessions.get(id);if(session){session.state=state;session.message=message}this.emit({type:'state',host:id,state,message,...(reason?{reason}:{})})}
  async connect(host,{password,keyPassphrase,remember=false,cols=100,rows=30,sessionId=host.id}={}){
    if(!host?.id||!host.address)throw new Error('Host identity is incomplete. Edit the host and check its address.');
    if(typeof host.user!=='string'||!host.user.trim())throw new Error('SSH username is missing. Edit the host or its identity and enter a username.');
    host={...host,credentialHostId:host.id,id:sessionId};
    if(host.jump)throw new Error('Jump-host connections are not available in this build yet.');
    const previous=this.sessions.get(host.id);
    if(previous&&['connected','connecting','pending'].includes(previous.state))return {state:previous.state};
    let credentials,keySecret;
    if(host.auth==='Password'){
      password=password??await this.vault.get(passwordReference(host));
      if(password===undefined)return {needsPassword:true};
      if(typeof password!=='string'||password.length>65536)throw new Error('Invalid password');
      credentials={password,authHandler:['password']};
    }else{
      if(!host.key)throw new Error('Choose an SSH key or use password authentication.');
      const serialized=await this.vault.get('key:'+host.key);
      if(!serialized)throw new Error('The selected private key is unavailable. Import it again or choose another key.');
      const key=JSON.parse(serialized);keySecret=key;const passphrase=keyPassphrase??key.passphrase;const parsed=utils.parseKey(key.privateKey,passphrase||undefined);if(parsed instanceof Error){if(keyPassphrase===undefined&&!key.passphrase)return {needsPassphrase:true};throw new Error('The SSH key passphrase is incorrect.')}credentials={privateKey:key.privateKey,passphrase:passphrase||undefined,authHandler:['publickey']};
    }
    const client=this.clientFactory();
    const s={client,host,state:'connecting',chunks:previous?.chunks||[],bytes:previous?.bytes||0,sequence:previous?.sequence||0,inflight:0,channel:null,done:false,cols,rows,message:''};
    this.sessions.set(host.id,s);this.state(host.id,'connecting');
    return new Promise((resolve,reject)=>{
      let settled=false;
      const fail=error=>{if(s.done)return;s.done=true;this.cancelVerifications(s);this.state(host.id,'disconnected',error.message||'SSH connection failed');client.destroy();if(!settled){settled=true;reject(new Error(error.message||'SSH connection failed'))}};
      client.on('error',fail);
      client.on('close',()=>{this.cancelVerifications(s);if(!s.done){s.done=true;this.state(host.id,'disconnected','Connection closed.')}if(!settled){settled=true;reject(new Error('Connection closed before the shell opened.'))}});
      client.on('ready',()=>{
        client.shell({term:'xterm-256color',cols:s.cols,rows:s.rows},async(error,channel)=>{
          if(error){fail(error);return}
          s.channel=channel;
          channel.on('data',chunk=>this.output(s,chunk));
          channel.stderr.on('data',chunk=>this.output(s,chunk));
          channel.on('error',fail);
          channel.on('exit',()=>{s.shellExited=true});
          channel.on('close',()=>{if(s.done)return;s.done=true;this.state(host.id,'disconnected','Remote shell closed.',s.shellExited?'shell-exit':'channel-closed');client.end()});
          if(remember&&host.auth!=='Password'&&keyPassphrase!==undefined){try{await this.vault.put('key:'+host.key,JSON.stringify({...keySecret,passphrase:keyPassphrase}))}catch(error){this.emit({type:'notice',host:host.id,message:error.message})}}
          if(remember&&host.auth==='Password'){
            try{await this.vault.put(passwordReference(host),password)}catch(error){this.emit({type:'notice',host:host.id,message:error.message})}
          }
          if(s.done)return;
          this.state(host.id,'connected');
          if(!settled){settled=true;resolve({state:'connected'})}
          if(host.startup)channel.write(host.startup+'\r');
        });
      });
      try{client.connect({host:host.address,port:Number(host.port||22),username:host.user,...credentials,readyTimeout:120000,keepaliveInterval:Number(host.keepalive??30)*1000,keepaliveCountMax:3,hostVerifier:(key,callback)=>{this.verify(s,key,callback).catch(fail)}})}catch(error){fail(error)}
    });
  }
  output(s,chunk){if(s.done)return;const buffer=Buffer.from(chunk);s.inflight+=buffer.length;if(s.inflight>128*1024){s.channel?.pause();s.channel?.stderr.pause()}s.chunks.push(buffer);s.bytes+=buffer.length;while(s.bytes>2*1024*1024&&s.chunks.length>1)s.bytes-=s.chunks.shift().length;this.emit({type:'data',host:s.host.id,sequence:++s.sequence,data:buffer.toString('base64')})}
  async verify(s,key,callback){
    const offered={fingerprint:fingerprint(key),algorithm:utils.parseKey(key)?.type||'SSH'};
    const known=await this.knownHosts.get(s.host);
    if(s.done){callback(false);return}
    if(known){
      if(known.fingerprint===offered.fingerprint){callback(true);return}
      this.state(s.host.id,'disconnected','Host key changed. The connection was blocked. Verify the server before updating known-hosts.json.');
      this.emit({type:'host-key-changed',host:s.host.id,...offered,previous:known.fingerprint});callback(false);return;
    }
    const requestId=randomUUID();
    const timer=setTimeout(()=>{this.verifications.delete(requestId);callback(false)},90000);timer.unref();
    this.verifications.set(requestId,{s,offered,callback,timer});this.state(s.host.id,'pending');
    this.emit({type:'host-key',host:s.host.id,requestId,...offered});
  }
  async respond(requestId,accept,remember){const pending=this.verifications.get(requestId);if(!pending)throw new Error('The host verification request has expired.');this.verifications.delete(requestId);clearTimeout(pending.timer);try{if(accept&&remember)await this.knownHosts.remember(pending.s.host,pending.offered);pending.callback(Boolean(accept))}catch(error){pending.callback(false);throw error}}
  cancelVerifications(s){for(const [id,p]of this.verifications){if(p.s===s){clearTimeout(p.timer);this.verifications.delete(id);p.callback(false)}}}
  write(id,data){const s=this.sessions.get(id);if(s?.state!=='connected'||!s.channel)return;if(typeof data!=='string'||data.length>65536)throw new Error('Invalid terminal input');s.channel.write(data)}
  acknowledge(id,bytes){if(!Number.isInteger(bytes)||bytes<0||bytes>4*1024*1024)return;const s=this.sessions.get(id);if(s){s.inflight=Math.max(0,s.inflight-bytes);if(s.inflight<64*1024){s.channel?.resume();s.channel?.stderr.resume()}}}
  resize(id,cols,rows){if(!Number.isInteger(cols)||!Number.isInteger(rows)||cols<2||rows<1||cols>1000||rows>1000)throw new Error('Invalid terminal dimensions');const s=this.sessions.get(id);if(s){s.cols=cols;s.rows=rows;s.channel?.setWindow(rows,cols,0,0)}}
  snapshot(id){const s=this.sessions.get(id);return s?{state:s.state,message:s.message,sequence:s.sequence,pendingBytes:s.inflight,data:Buffer.concat(s.chunks).toString('base64')}:{state:'not-connected',data:''}}
  disconnect(id){const s=this.sessions.get(id);if(!s)return;s.done=true;this.cancelVerifications(s);s.channel?.close();s.client.destroy();this.state(id,'disconnected','Disconnected.');this.sessions.delete(id)}
  dispose(){for(const id of this.sessions.keys())this.disconnect(id)}
}
export {fingerprint};
