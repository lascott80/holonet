import ssh2 from 'ssh2';
const {utils}=ssh2;
import {promisify} from 'node:util';
import {fingerprint} from './ssh.mjs';
import {keyMetadata} from './secrets.mjs';
const generate=promisify(utils.generateKeyPair);
export async function importKey(vault,{name,privateKey,passphrase='',remember=false}){
 if(typeof name!=='string'||!name.trim()||name.length>512||typeof privateKey!=='string'||privateKey.length>1024*1024||typeof passphrase!=='string'||passphrase.length>65536)throw new Error('Invalid SSH key input.');
 const parsed=utils.parseKey(privateKey,passphrase||undefined);
 if(parsed instanceof Error)throw new Error('The private key could not be read. Check the file and passphrase.');
 const key=Array.isArray(parsed)?parsed[0]:parsed;
 if(!key?.isPrivateKey())throw new Error('Choose a private SSH key, not a public key.');
 const metadata={...keyMetadata(key,name.trim()),fingerprint:fingerprint(key.getPublicSSH())};
 await vault.put('key:'+metadata.id,JSON.stringify({privateKey,passphrase:remember?passphrase:'',publicKey:metadata.publicKey}));
 return metadata;
}
export async function generateKey(vault,{name,type='ED25519',bits='4096',comment='',passphrase=''}){
 if(!['ED25519','RSA'].includes(type)||!['2048','3072','4096'].includes(String(bits))||typeof comment!=='string'||comment.length>4096||typeof passphrase!=='string'||passphrase.length>65536)throw new Error('Invalid key options.');
 const options={comment,...(type==='RSA'?{bits:Number(bits)}:{}),...(passphrase?{passphrase,cipher:'aes256-cbc'}:{})};
 const pair=await generate(type==='RSA'?'rsa':'ed25519',options);
 return importKey(vault,{name,privateKey:pair.private,passphrase});
}
export async function publicKey(vault,id){const data=await vault.get('key:'+id);if(!data)throw new Error('Key not found.');const secret=JSON.parse(data);if(secret.publicKey)return secret.publicKey;const parsed=utils.parseKey(secret.privateKey,secret.passphrase||undefined);if(parsed instanceof Error)throw new Error('Key could not be unlocked.');const key=Array.isArray(parsed)?parsed[0]:parsed;return keyMetadata(key,'').publicKey}
