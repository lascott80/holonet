import {sanitizeState} from '../shared/state.mjs';
import {initialHosts,initialGroups,initialKeys,initialSettings,defaultBindings} from './data.js';
const storageKey='holonet.preview.state.v1';
let pending=Promise.resolve(),lastRequested,latestState;
export async function loadState(){
  if(window.holonetDesktop)return window.holonetDesktop.load();
  const saved=localStorage.getItem(storageKey);
  return {runtime:'browser',state:saved?sanitizeState(JSON.parse(saved)):{version:1,hosts:initialHosts,groups:initialGroups,keyMetadata:initialKeys,settings:initialSettings,bindings:defaultBindings,sessionTabs:[]}};
}
export function saveState(snapshot){
  const state=sanitizeState(snapshot),serialized=JSON.stringify(state);
  latestState=state;
  if(serialized===lastRequested)return pending;
  lastRequested=serialized;
  const operation=pending.catch(()=>{}).then(()=>window.holonetDesktop?window.holonetDesktop.save(state):localStorage.setItem(storageKey,serialized));
  pending=operation;
  operation.catch(()=>{if(lastRequested===serialized)lastRequested=undefined});
  return operation;
}
export function flushState(){return pending.catch(()=>latestState?saveState(latestState):Promise.resolve())}
