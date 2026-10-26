import test from 'node:test';
import assert from 'node:assert/strict';
import {customizeMachOUUID} from '../scripts/macos-identity.mjs';
function fixture(){const data=Buffer.alloc(80,0xaa);data.writeUInt32LE(0xfeedfacf,0);data.writeUInt32LE(1,16);data.writeUInt32LE(24,20);data.writeUInt32LE(0x1b,32);data.writeUInt32LE(24,36);return data}
test('app UUID differs from Electron and helpers; only UUID bytes change',()=>{
 const original=fixture(),a=customizeMachOUUID(original,'app.holonet.desktop:main'),b=customizeMachOUUID(original,'app.holonet.desktop:helper');
 assert.notDeepEqual(a.subarray(40,56),original.subarray(40,56));assert.notDeepEqual(a.subarray(40,56),b.subarray(40,56));
 assert.deepEqual(a.subarray(0,40),original.subarray(0,40));assert.deepEqual(a.subarray(56),original.subarray(56));
 assert.deepEqual(customizeMachOUUID(original,'app.holonet.desktop:main'),a);
});
test('unexpected or malformed executables fail packaging instead of corrupting a binary',()=>{
 assert.throws(()=>customizeMachOUUID(Buffer.alloc(8),'x'),/Mach-O/);
 const bad=fixture();bad.writeUInt32LE(1000,36);assert.throws(()=>customizeMachOUUID(bad,'x'),/size/);
 const missing=fixture();missing.writeUInt32LE(0,16);assert.throws(()=>customizeMachOUUID(missing,'x'),/no LC_UUID/);
});
