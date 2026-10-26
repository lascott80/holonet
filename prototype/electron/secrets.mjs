import {readFile,mkdir} from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {atomicWrite} from './store.mjs';

export class PrivateDocument {
  #tail=Promise.resolve();
  constructor(filename,defaults){this.filename=filename;this.defaults=defaults}
  async read(){try{const text=await readFile(this.filename,'utf8');if(Buffer.byteLength(text)>8*1024*1024)throw new Error('Vault is too large');const data=JSON.parse(text);if(data.version!==1||!data.entries||typeof data.entries!=='object'||Array.isArray(data.entries))throw new Error('Invalid vault');return data}catch(error){if(error.code==='ENOENT')return structuredClone(this.defaults);throw new Error('Saved credential data could not be read; the original file is unchanged.')}}
  update(callback){const operation=this.#tail.catch(()=>{}).then(async()=>{const data=await this.read();const result=await callback(data);await mkdir(path.dirname(this.filename),{recursive:true,mode:0o700});await atomicWrite(this.filename,JSON.stringify(data)+'\n');return result});this.#tail=operation;return operation}
  flush(){return this.#tail}
}
export class CredentialVault {
  constructor(directory,provider){this.provider=provider;this.doc=new PrivateDocument(path.join(directory,'credentials.encrypted.json'),{version:1,entries:{}})}
  async put(id,secret){if(typeof id!=='string'||id.length>512||!id||typeof secret!=='string'||secret.length>1024*1024)throw new Error('Invalid credential');if(!await this.provider.available())throw new Error('OS credential encryption is unavailable. Connect without remembering the password.');const encrypted=await this.provider.encrypt(secret);await this.doc.update(data=>{data.entries[id]=Buffer.from(encrypted).toString('base64')})}
  async get(id){await this.doc.flush();const entry=(await this.doc.read()).entries[id];if(entry===undefined)return undefined;if(typeof entry!=='string')throw new Error('Invalid encrypted credential');if(!await this.provider.available())throw new Error('OS credential encryption is unavailable.');return this.provider.decrypt(Buffer.from(entry,'base64'))}
  remove(id){return this.doc.update(data=>{delete data.entries[id]})}
  flush(){return this.doc.flush()}
}
export class KnownHosts {
  constructor(directory){this.doc=new PrivateDocument(path.join(directory,'known-hosts.json'),{version:1,entries:{}})}
  identity(host){return JSON.stringify([host.address.trim().toLowerCase(),Number(host.port||22)])}
  async get(host){await this.doc.flush();return (await this.doc.read()).entries[this.identity(host)]}
  remember(host,key){return this.doc.update(data=>{data.entries[this.identity(host)]=key})}
}
export function keyMetadata(parsed,name,id=randomUUID()){
  const publicKey=parsed.getPublicSSH();
  return {id,name,type:parsed.type==='ssh-ed25519'?'ED25519':parsed.type,comment:parsed.comment||'',publicKey:parsed.type+' '+publicKey.toString('base64')};
}
