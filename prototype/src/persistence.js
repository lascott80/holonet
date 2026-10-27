import {sanitizeState} from '../shared/state.mjs';
import {initialHosts,initialGroups,initialKeys,initialSettings,defaultBindings} from './data.js';
const storageKey='holonet.preview.state.v1';
let pending=Promise.resolve(),lastRequested,latestState,queuedState,writing=false;
export async function loadState(){
  if(window.holonetDesktop)return window.holonetDesktop.load();
  const saved=localStorage.getItem(storageKey);
  return {runtime:'browser',state:saved?sanitizeState(JSON.parse(saved)):{version:1,hosts:initialHosts,groups:initialGroups,keyMetadata:initialKeys,settings:initialSettings,bindings:defaultBindings,sessionTabs:[]}};
}
export function saveState(snapshot){
  const state=sanitizeState({...snapshot,sessionTabs:snapshot.settings.rememberTabs?snapshot.sessionTabs:[]}),serialized=JSON.stringify(state);
  latestState=state;
  if(serialized===lastRequested)return pending;
  lastRequested=serialized;queuedState=state;
  if(!writing){
    writing=true;
    pending=Promise.resolve().then(async()=>{
      try{while(queuedState){const next=queuedState;queuedState=undefined;await (window.holonetDesktop?window.holonetDesktop.save(next):localStorage.setItem(storageKey,JSON.stringify(next)))}}catch(error){lastRequested=undefined;throw error}finally{writing=false}
    });
  }
  return pending;
}
export function flushState(){return pending.catch(()=>latestState?saveState(latestState):Promise.resolve())}
