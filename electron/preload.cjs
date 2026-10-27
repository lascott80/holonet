const {contextBridge, ipcRenderer} = require('electron');
contextBridge.exposeInMainWorld('holonetDesktop', {
  platform: process.platform,
  updateTokenStatus:()=>ipcRenderer.invoke('holonet:update-token-status'),
  updateTokenSave:token=>ipcRenderer.invoke('holonet:update-token-save',token),
  updateTokenRemove:()=>ipcRenderer.invoke('holonet:update-token-remove'),
  tool: request=>ipcRenderer.invoke('holonet:tool',request),
  onToolEvent: callback=>{const handler=(_event,data)=>callback(data);ipcRenderer.on('holonet:tool-event',handler);return()=>ipcRenderer.removeListener('holonet:tool-event',handler)},
  clipboardRead: () => ipcRenderer.invoke('holonet:clipboard-read'),
  clipboardWrite: text => ipcRenderer.invoke('holonet:clipboard-write',text),
  terminalFocus: id => ipcRenderer.send('holonet:terminal-focus',id),
  terminalMenu: request => ipcRenderer.send('holonet:terminal-menu',request),
  onTerminalEdit: callback => {const handler=(_event,data)=>callback(data);ipcRenderer.on('holonet:terminal-edit',handler);return()=>ipcRenderer.removeListener('holonet:terminal-edit',handler)},
  sshConnect: request => ipcRenderer.invoke('holonet:ssh-connect',request),
  sshVerify: request => ipcRenderer.invoke('holonet:ssh-verify',request),
  sshDisconnect: id => ipcRenderer.invoke('holonet:ssh-disconnect',id),
  sshSnapshot: id => ipcRenderer.invoke('holonet:ssh-snapshot',id),
  sshWrite: (id,data) => ipcRenderer.send('holonet:ssh-write',id,data),
  sshAck: (id,bytes) => ipcRenderer.send('holonet:ssh-ack',id,bytes),
  sshResize: (id,cols,rows) => ipcRenderer.send('holonet:ssh-resize',id,cols,rows),
  onSSHEvent: callback => {const handler=(_event,data)=>callback(data);ipcRenderer.on('holonet:ssh-event',handler);return()=>ipcRenderer.removeListener('holonet:ssh-event',handler)},
  forgetHostCredentials: id => ipcRenderer.invoke('holonet:forget-host',id),
  keyImport: data => ipcRenderer.invoke('holonet:key-import',data),
  keyGenerate: data => ipcRenderer.invoke('holonet:key-generate',data),
  keyPublic: id => ipcRenderer.invoke('holonet:key-public',id),
  load: () => ipcRenderer.invoke('holonet:load'),
  save: state => ipcRenderer.invoke('holonet:save', state),
  ready: () => ipcRenderer.send('holonet:ready'),
  onCloseRequested: callback => {
    const handler = async () => {
      try { await callback(); ipcRenderer.send('holonet:close-ready'); }
      catch { ipcRenderer.send('holonet:close-failed'); }
    };
    ipcRenderer.on('holonet:close-request', handler);
    return () => ipcRenderer.removeListener('holonet:close-request', handler);
  },
});
