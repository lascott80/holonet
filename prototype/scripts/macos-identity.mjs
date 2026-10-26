import {createHash} from 'node:crypto';
import {readFile,writeFile,readdir} from 'node:fs/promises';
import path from 'node:path';

// Renaming/signing Electron leaves LC_UUID unchanged. macOS Local Network
// privacy also uses this UUID, so each Holonet executable needs its own identity.
export function customizeMachOUUID(source,identity) {
  const data=Buffer.from(source);
  if(data.length<32||data.readUInt32LE(0)!==0xfeedfacf)throw new Error('Expected a thin 64-bit Mach-O executable');
  const count=data.readUInt32LE(16),end=32+data.readUInt32LE(20);
  if(end>data.length)throw new Error('Invalid Mach-O load command region');
  let offset=32;
  for(let i=0;i<count;i++){
    if(offset+8>end)throw new Error('Truncated Mach-O load command');
    const command=data.readUInt32LE(offset),size=data.readUInt32LE(offset+4);
    if(size<8||offset+size>end)throw new Error('Invalid Mach-O load command size');
    if(command===0x1b){
      if(size!==24)throw new Error('Invalid LC_UUID size');
      const uuid=createHash('sha256').update(identity).update(data.subarray(offset+8,offset+24)).digest().subarray(0,16);
      uuid[6]=(uuid[6]&0x0f)|0x80;uuid[8]=(uuid[8]&0x3f)|0x80;
      uuid.copy(data,offset+8);return data;
    }
    offset+=size;
  }
  throw new Error('Executable has no LC_UUID');
}

export default async function afterPack(context){
  if(context.electronPlatformName!=='darwin')return;
  const app=path.join(context.appOutDir,context.packager.appInfo.productFilename+'.app');
  const frameworks=path.join(app,'Contents','Frameworks');
  const bundles=[app,...(await readdir(frameworks,{withFileTypes:true})).filter(entry=>entry.isDirectory()&&entry.name.endsWith('.app')).map(entry=>path.join(frameworks,entry.name))];
  for(const bundle of bundles){
    const executables=path.join(bundle,'Contents','MacOS');
    for(const entry of await readdir(executables,{withFileTypes:true})){
      if(!entry.isFile())continue;
      const filename=path.join(executables,entry.name);
      const identity=context.packager.appInfo.id+':'+path.relative(app,filename);
      await writeFile(filename,customizeMachOUUID(await readFile(filename),identity));
    }
  }
}
