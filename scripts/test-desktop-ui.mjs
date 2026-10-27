// Real Electron UI with disposable data and a local SSH server.
import {_electron as electron} from 'playwright-core';
import executablePath from 'electron';
import {mkdtemp,mkdir,writeFile,readFile,rm} from 'node:fs/promises';
import path from 'node:path';import os from 'node:os';import assert from 'node:assert/strict';
import {initialSettings,initialGroups,defaultBindings} from '../src/data.js';
import {toolsServer} from '../tests/helpers/tools-server.mjs';
const dir=await mkdtemp(path.join(os.tmpdir(),'holonet-ui-')),remote=path.join(dir,'remote');await mkdir(remote);await writeFile(path.join(remote,'fixture.txt'),'test');const server=await toolsServer(remote);let app;
const launch=async()=>{app=await electron.launch({executablePath,args:['.'],env:{...process.env,HOLONET_USER_DATA:dir},timeout:30000});const page=await app.firstWindow();await page.getByRole('heading',{name:'Hosts',exact:true}).first().waitFor();return page};
try{
 await mkdir(path.join(dir,'holonet'),{recursive:true});
 await writeFile(path.join(dir,'holonet','state.json'),JSON.stringify({version:1,hosts:[],identities:[{id:'ui-identity',name:'UI identity',user:'tester',auth:'Password',key:''}],groups:initialGroups,keyMetadata:[],settings:{...initialSettings,confirmQuit:false,confirmClose:false},bindings:defaultBindings,sessionTabs:[]}));
 let page=await launch();
 await page.getByRole('button',{name:'New host'}).first().click();await page.getByLabel('Label',{exact:true}).fill('UI fixture');await page.getByLabel('Address',{exact:true}).fill('127.0.0.1');await page.getByLabel('Port',{exact:true}).fill(String(server.port));await page.getByLabel('Identity',{exact:true}).selectOption('ui-identity');await page.getByRole('button',{name:'Save host'}).click();
 await page.getByRole('button',{name:'Connect',exact:true}).last().click();
 await page.getByLabel('Password',{exact:true}).fill('fixture');await page.getByRole('button',{name:'Connect',exact:true}).last().click();
 await page.getByRole('dialog',{name:'Verify host key'}).waitFor();await page.getByRole('button',{name:'Trust & connect'}).click();
 await page.getByRole('tab',{name:/UI fixture · connected/}).waitFor();
 await page.getByRole('button',{name:'SFTP',exact:true}).click();await page.getByRole('button',{name:/fixture.txt/}).waitFor();
 await page.getByLabel('Filter remote files').fill('fixture');await page.getByRole('button',{name:/fixture.txt/}).click();await page.getByRole('button',{name:'Permissions',exact:true}).last().click();await page.getByRole('dialog',{name:'Edit permissions'}).waitFor();await page.getByRole('button',{name:'Cancel',exact:true}).last().click();
 await page.getByRole('button',{name:'Close UI fixture session'}).click();
 await page.getByRole('button',{name:'Connection history'}).click();await page.getByText('UI fixture',{exact:true}).first().waitFor();
 await page.getByRole('button',{name:'Clear history'}).click();await page.getByRole('button',{name:'Continue'}).click();await page.getByText('No connections recorded').waitFor();
 await page.getByRole('button',{name:'Settings',exact:true}).click();await page.getByRole('button',{name:'General',exact:true}).click();assert.equal(await page.getByRole('switch',{name:'Reopen previous tabs'}).isChecked(),false);
 const hasUpdateMenu=await app.evaluate(({Menu})=>Menu.getApplicationMenu().items.some(item=>item.submenu?.items.some(entry=>entry.label==='Check for Updates…')));assert.equal(hasUpdateMenu,true);
 await page.getByLabel('GitHub token',{exact:true}).fill('fake-ui-only-token');await page.getByRole('button',{name:'Save token'}).click();await page.getByText('GitHub token saved.').waitFor();assert.doesNotMatch(await readFile(path.join(dir,'holonet','credentials.encrypted.json'),'utf8'),/fake-ui-only-token/);await page.getByRole('button',{name:'Remove token'}).click();await page.getByText('GitHub token removed.').waitFor();
 await page.getByRole('button',{name:'Hosts',exact:true}).last().click();await page.getByRole('button',{name:'Connect',exact:true}).last().click();await page.getByLabel('Password',{exact:true}).fill('fixture');await page.getByRole('button',{name:'Connect',exact:true}).last().click();await page.getByRole('tab',{name:/UI fixture · connected/}).waitFor();
 await app.close();app=null;
 page=await launch();assert.equal(await page.getByRole('tab',{name:/UI fixture/}).count(),0);
 await page.getByRole('button',{name:'Settings',exact:true}).click();await page.getByRole('button',{name:'General',exact:true}).click();await page.getByRole('switch',{name:'Reopen previous tabs'}).check();await page.getByRole('button',{name:'Hosts',exact:true}).last().click();await page.getByRole('button',{name:'Connect',exact:true}).last().click();await page.getByLabel('Password',{exact:true}).fill('fixture');await page.getByRole('button',{name:'Connect',exact:true}).last().click();await page.getByRole('tab',{name:/UI fixture · connected/}).waitFor();
 await app.close();app=null;page=await launch();await page.getByRole('tab',{name:/UI fixture · not connected/}).waitFor();
 console.log('Desktop UI: identity, SSH reconnect, host-key trust, SFTP filtering and permissions, encrypted update-token management, update menu, clear history, and both tab restoration settings passed.');
}finally{if(app)await app.close().catch(()=>{});await server.close();await rm(dir,{recursive:true,force:true})}
