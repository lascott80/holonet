import {app, BrowserWindow, dialog, ipcMain, Menu, safeStorage, clipboard, shell} from 'electron';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {CredentialVault,KnownHosts,PrivateDocument} from './secrets.mjs';
import {DesktopTools} from './tools.mjs';
import {effectiveHost,accelerator} from '../shared/model.mjs';
import {SSHSessions} from './ssh.mjs';
import {generateKey,importKey,publicKey} from './keys.mjs';
import {StateStore} from './store.mjs';
import {checkForUpdates,updateTokenStatus,saveUpdateToken,removeUpdateToken} from './updates.mjs';
import {initialGroups, initialSettings, bindingsForPlatform} from '../src/data.js';
const here = path.dirname(fileURLToPath(import.meta.url));
const smoke = process.argv.includes('--smoke-test');
app.setName('Holonet');
if (process.env.HOLONET_USER_DATA) app.setPath('userData', path.resolve(process.env.HOLONET_USER_DATA));
if(!app.requestSingleInstanceLock())app.exit(0);
app.on('second-instance',()=>{if(window){if(window.isMinimized())window.restore();window.show();window.focus()}});
let window, store, vault, ssh, tools, knownHosts, windowPrefs, rendererURL, closing = false;
let closeRequested = false;
let terminalTarget = null;
function terminalEdit(action, hostId = terminalTarget) {
  if (!window || window.isDestroyed()) return;
  if (hostId) window.webContents.send('holonet:terminal-edit', {hostId, action});
  else if (['copy','paste','selectAll'].includes(action)) window.webContents[action]();
}
const validHostId = id => typeof id === 'string' && id.length > 0 && id.length <= 512;
let quitting = false, rendererReady = false, smokeDone = false;
const desktopBindings=bindingsForPlatform(process.platform);
const defaults = {version:1,hosts:[],groups:initialGroups,keyMetadata:[],settings:initialSettings,bindings:desktopBindings,sessionTabs:[]};
function authorized(event) {
  if (!window || event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame || event.senderFrame.url !== rendererURL) throw new Error('Unauthorized desktop request');
}
async function createWindow() {
  terminalTarget = null;
  closing = false;
  rendererReady = false;
  closeRequested = false;
  const prefs=await store.load(),savedBounds=prefs.settings.window?(await windowPrefs.read()).entries.bounds:null;
  window = new BrowserWindow({width:1440,height:960,...savedBounds,minWidth:780,minHeight:600,title:'Holonet',...(process.platform==='darwin'?{titleBarStyle:'hiddenInset',trafficLightPosition:{x:14,y:15}}:process.platform==='win32'?{titleBarStyle:'hidden',titleBarOverlay:{color:'#282828',symbolColor:'#dddddd',height:48}}:{}),backgroundColor:'#242424',show:false,webPreferences:{preload:path.join(here,'preload.cjs'),contextIsolation:true,sandbox:true,nodeIntegration:false,webSecurity:true}});
  if(process.env.HOLONET_USER_DATA)window.webContents.on('console-message',details=>{if(details.level==='error')console.error('RENDERER_ERROR',details.message)});
  window.webContents.setWindowOpenHandler(() => ({action:'deny'}));
  window.webContents.on('will-navigate', (event, url) => {if (url !== rendererURL) event.preventDefault();});
  window.webContents.session.setPermissionRequestHandler((_contents,_permission,callback) => callback(false));
  window.on('close', async event => {
    if(window && !window.isMaximized())windowPrefs.update(d=>{d.entries.bounds=window.getBounds()}).catch(()=>{});
    if (closing || smoke) return;
    event.preventDefault();
    if(closeRequested)return;
    closeRequested=true;
    try{
      const preferences=(await store.load()).settings;
      if(preferences.confirmQuit&&[...ssh.sessions.values()].some(session=>session.state==='connected')){
        const choice=await dialog.showMessageBox(window,{type:'question',buttons:['Cancel','Disconnect and close'],defaultId:0,cancelId:0,message:'Close active SSH sessions?',detail:'Closing this window will disconnect its SSH sessions.'});
        if(choice.response===0){closeRequested=false;quitting=false;return}
      }
    }catch{closeRequested=false;quitting=false;dialog.showErrorBox('Unable to close Holonet','Saved settings could not be read. The window will stay open.');return}
    if(!rendererReady){store.flush().then(()=>{closing=true;window?.close()}).catch(()=>dialog.showErrorBox('Settings could not be saved','Holonet will stay open.'));return}
    window.webContents.send('holonet:close-request');
  });
  window.webContents.on('render-process-gone',()=>{rendererReady=false;tools?.dispose();ssh?.dispose()});
  window.webContents.on('did-start-navigation',(_event,_url,_inPlace,isMain)=>{if(isMain){tools?.dispose();ssh?.dispose()}});
  window.on('closed',()=>{tools?.dispose();ssh?.dispose();window=null;if(quitting)app.quit()});
  window.once('ready-to-show',()=>{if(!smoke)window.show()});
  const devURL = process.env.HOLONET_DEV_URL;
  if (devURL) {
    const parsed = new URL(devURL);
    if (!['http:', 'https:'].includes(parsed.protocol) || !['localhost','127.0.0.1'].includes(parsed.hostname)) throw new Error('Desktop development URL must be loopback');
    rendererURL=parsed.href;
    await window.loadURL(rendererURL);
  } else {
    rendererURL=pathToFileURL(path.resolve(here,'../dist/client/index.html')).href;
    await window.loadFile(path.resolve(here,'../dist/client/index.html'));
  }
}
ipcMain.handle('holonet:load',event=>{authorized(event);return store.load().then(state=>({runtime:'desktop',version:app.getVersion(),state}))});
ipcMain.handle('holonet:save',async(event,state)=>{authorized(event);const before=await store.load();await store.save(state);refreshMenu(state.bindings);if(!smoke && app.isPackaged && before.settings.login!==state.settings.login && ['darwin','win32'].includes(process.platform))app.setLoginItemSettings({openAtLogin:state.settings.login});return {saved:true}});
ipcMain.on('holonet:close-ready',async event=>{authorized(event);try{await store.flush();await vault?.flush();tools?.dispose();ssh?.dispose();closing=true;window?.close()}catch{closeRequested=false;quitting=false;dialog.showErrorBox('Settings could not be saved','Holonet will stay open so you can try again.')}});
ipcMain.on('holonet:close-failed',event=>{authorized(event);closeRequested=false;quitting=false;dialog.showErrorBox('Settings could not be saved','Holonet will stay open so you can try again.')});
ipcMain.on('holonet:ready', async event=>{authorized(event);rendererReady=true;if(smoke&&!smokeDone){smokeDone=true;await store.flush();console.log('HOLONET_SMOKE_OK: isolated renderer and persistent state loaded');setTimeout(()=>app.exit(0),100)}});

ipcMain.on('holonet:terminal-focus',(event,id)=>{
  authorized(event);terminalTarget=validHostId(id)?id:null;
});
ipcMain.handle('holonet:clipboard-read',event=>{authorized(event);return clipboard.readText()});
ipcMain.handle('holonet:clipboard-write',(event,text)=>{
  authorized(event);if(typeof text!=='string'||text.length>16*1024*1024)throw new Error('Clipboard text is too large');
  return clipboard.writeText(text);
});
ipcMain.on('holonet:terminal-menu',(event,request)=>{
  authorized(event);if(!request||!validHostId(request.hostId))return;
  const {hostId,hasSelection}=request;
  Menu.buildFromTemplate([
    {label:'Copy',enabled:hasSelection===true,click:()=>terminalEdit('copy',hostId)},
    {label:'Paste',click:()=>terminalEdit('paste',hostId)},
    {type:'separator'},
    {label:'Select All',click:()=>terminalEdit('selectAll',hostId)},
    {label:'Clear',click:()=>terminalEdit('clear',hostId)},
  ]).popup({window});
});
ipcMain.handle('holonet:ssh-connect',async(event,request)=>{
 authorized(event);if(!request||typeof request.hostId!=='string'||typeof request.remember!=='boolean'&&request.remember!==undefined)throw new Error('Invalid connection request');
 const state=await store.load(),saved=state.hosts.find(h=>h.id===request.hostId);if(!saved)throw new Error('Save the host before connecting.');
 if(request.sessionId!==undefined&&!validHostId(request.sessionId))throw Error('Invalid session');
 const host=effectiveHost(saved,state.groups,state.identities);
 return ssh.connect(host,{sessionId:request.sessionId||host.id,password:request.password,keyPassphrase:request.password,remember:request.remember});
});
ipcMain.handle('holonet:ssh-verify',(event,request)=>{authorized(event);if(!request||typeof request.requestId!=='string'||typeof request.accept!=='boolean'||typeof request.remember!=='boolean')throw new Error('Invalid verification');return ssh.respond(request.requestId,request.accept,request.remember)});
ipcMain.handle('holonet:ssh-snapshot',(event,id)=>{authorized(event);return ssh.snapshot(id)});
ipcMain.handle('holonet:ssh-disconnect',(event,id)=>{authorized(event);ssh.disconnect(id)});
for(const channel of ['write','resize','ack'])ipcMain.on('holonet:ssh-'+channel,(event,id,...args)=>{try{authorized(event);if(channel==='write')ssh.write(id,args[0]);else if(channel==='resize')ssh.resize(id,args[0],args[1]);else ssh.acknowledge(id,args[0])}catch{}});
ipcMain.handle('holonet:forget-host',async(event,id)=>{authorized(event);if(typeof id!=='string'||!id||id.length>512)throw new Error('Invalid host');return vault.doc.update(data=>{for(const key of Object.keys(data.entries))if(key.startsWith('host:'+id+':'))delete data.entries[key]})});
ipcMain.handle('holonet:key-import',(event,data)=>{authorized(event);return importKey(vault,data)});
ipcMain.handle('holonet:key-generate',(event,data)=>{authorized(event);return generateKey(vault,data)});
ipcMain.handle('holonet:key-public',(event,id)=>{authorized(event);if(typeof id!=='string'||id.length>512)throw new Error('Invalid key');return publicKey(vault,id)});
let updateCheckPending=false;
const updateDialog=options=>window&&!window.isDestroyed()?dialog.showMessageBox(window,options):dialog.showMessageBox(options);
async function showUpdateCheck(){
 if(updateCheckPending)return;updateCheckPending=true;
 try{
  const result=await checkForUpdates({vault,currentVersion:app.getVersion(),platform:process.arch==='arm64'?process.platform:''});
  if(!result.available){await updateDialog({type:'info',title:'Check for Updates',message:'Holonet is up to date',detail:`Current version: ${result.currentVersion}`,buttons:['OK']});return}
  const detail=result.hasInstaller?'Open GitHub to download the installer. Quit Holonet before installing; active SSH sessions will disconnect.':'The latest release does not have an installer for this platform yet. You can review the release on GitHub.';
  const choice=await updateDialog({type:'info',title:'Check for Updates',message:`Holonet ${result.version} is available`,detail,buttons:['Open release','Cancel'],defaultId:0,cancelId:1,noLink:true});
  if(choice.response===0)await shell.openExternal(result.url);
 }catch(error){await updateDialog({type:'error',title:'Update check failed',message:'Could not check for updates',detail:error.message,buttons:['OK']})}finally{updateCheckPending=false}
}
ipcMain.handle('holonet:update-token-status',event=>{authorized(event);return updateTokenStatus(vault)});
ipcMain.handle('holonet:update-token-save',(event,token)=>{authorized(event);return saveUpdateToken(vault,token)});
ipcMain.handle('holonet:update-token-remove',event=>{authorized(event);return removeUpdateToken(vault)});
function refreshMenu(bindings){
 const mac=process.platform==='darwin';
 const shortcut=(action,fallback)=>{const found=bindings.find(b=>b[1]===action)?.[2];return found?accelerator(found):fallback};
 Menu.setApplicationMenu(Menu.buildFromTemplate([
 ...(mac?[{role:'appMenu'}]:[]),
 {label:'Edit',submenu:[{role:'undo'},{role:'redo'},{type:'separator'},{role:'cut'},
 {label:'Copy',accelerator:shortcut('Copy selection',mac?'Cmd+C':'Ctrl+Shift+C'),click:()=>terminalEdit('copy')},
 {label:'Paste',accelerator:shortcut('Paste',mac?'Cmd+V':'Ctrl+Shift+V'),click:()=>terminalEdit('paste')},
 {label:'Select All',accelerator:shortcut('Select all',mac?'Cmd+A':'Ctrl+Shift+A'),click:()=>terminalEdit('selectAll')}]},
 {role:'viewMenu'},{role:'windowMenu'},
 {role:'help',submenu:[{label:'Check for Updates…',click:()=>showUpdateCheck()}]}]));
}
ipcMain.handle('holonet:tool',async(event,r)=>{
 authorized(event);if(!r||typeof r!=='object')throw Error('Invalid tool request');
 switch(r.action){
 case 'list':return tools.list(r);
 case 'file':return tools.fileAction(r);
 case 'transfer':if(!['upload','download'].includes(r.direction)||typeof r.replace!=='boolean')throw Error('Invalid transfer');return tools.transfer(r);
 case 'key-remove':if(typeof r.id!=='string'||r.id.length>512)throw Error('Invalid key');return vault.remove('key:'+r.id);
 case 'cancel':return tools.cancel(r.id);
 case 'forward-start':{const state=await store.load(),rule=state.forwardings?.find(x=>x.id===r.id);if(!rule)throw Error('Rule not found');const session=tools.session(r.sessionId);if((session.host.credentialHostId||session.host.id)!==rule.host)throw Error('Choose a session for the rule host');return tools.startForward(rule,r.sessionId)}
 case 'forward-stop':return tools.stopForward(r.id);
 case 'forward-status':return [...tools.tunnels.keys()];
 case 'known-list':{await knownHosts.doc.flush();return Object.entries((await knownHosts.doc.read()).entries).map(([id,key])=>({id,address:JSON.parse(id)[0],port:JSON.parse(id)[1],...key}))}
 case 'known-remove':if(typeof r.id!=='string'||r.id.length>1024)throw Error('Invalid trust record');return knownHosts.doc.update(d=>{delete d.entries[r.id]});
 case 'history-clear':return tools.clearHistory();
 case 'history':await tools.history.flush();return Object.values((await tools.history.read()).entries).sort((a,b)=>b.updated-a.updated);
 default:throw Error('Unsupported tool');
 }
});
app.on('before-quit',event=>{if(window&&!closing&&!smoke){event.preventDefault();quitting=true;window.close()}});
app.whenReady().then(async()=>{
  store=new StateStore(path.join(app.getPath('userData'),'holonet'),defaults);
  await store.load();
  const directory=path.join(app.getPath('userData'),'holonet');
  vault=new CredentialVault(directory,{
    available:()=>safeStorage.isAsyncEncryptionAvailable(),
    encrypt:value=>safeStorage.encryptStringAsync(value),
    decrypt:async buffer=>{const decoded=await safeStorage.decryptStringAsync(buffer);return decoded.result},
  });
  knownHosts=new KnownHosts(directory);
  windowPrefs=new PrivateDocument(path.join(directory,'window.json'),{version:1,entries:{}});
  ssh=new SSHSessions({vault,knownHosts,emit:event=>{tools?.record(event).catch(()=>{});if(window&&!window.isDestroyed())window.webContents.send('holonet:ssh-event',event)}});
  tools=new DesktopTools({ssh,knownHosts,directory,emit:event=>{if(window&&!window.isDestroyed())window.webContents.send('holonet:tool-event',event)}});

  refreshMenu((await store.load()).bindings);
  await createWindow();
  app.on('activate',()=>{if(!window)createWindow().catch(error=>dialog.showErrorBox('Unable to open Holonet',error.message))});
}).catch(error=>{console.error(error);if(!smoke)dialog.showErrorBox('Unable to open Holonet',error.message);app.exit(1)});
app.on('window-all-closed',()=>{if(process.platform!=='darwin')app.quit()});
if(smoke)setTimeout(()=>{console.error('HOLONET_SMOKE_TIMEOUT');app.exit(1)},20000).unref();
