import React,{useEffect,useRef,useState} from 'react';
import {Terminal} from '@xterm/xterm';
import {SearchAddon} from '@xterm/addon-search';
import {FitAddon} from '@xterm/addon-fit';
import {WebglAddon} from '@xterm/addon-webgl';
import {createTerminalRenderer} from './terminal-renderer';
import '@xterm/xterm/css/xterm.css';
import {editTerminal} from './terminal-edit';
import {terminalFontFamily} from './terminal-font';
import {chord} from '../shared/model.mjs';
const bytes=base64=>Uint8Array.from(atob(base64),c=>c.charCodeAt(0));
export function NativeTerminal({hostId,active,settings,bindings,register,onResults}){
 const container=useRef(),terminal=useRef(),fit=useRef(),renderer=useRef(),options=useRef({settings,bindings}),[error,setError]=useState('');options.current={settings,bindings};
 useEffect(()=>{
  const bridge=window.holonetDesktop,term=new Terminal({fontFamily:terminalFontFamily(settings.font),letterSpacing:0,customGlyphs:true,fontSize:settings.fontSize,lineHeight:Number(settings.lineHeight),cursorBlink:settings.blink,cursorStyle:settings.cursor.toLowerCase(),scrollback:settings.scrollback,screenReaderMode:true,allowProposedApi:true,macOptionIsMeta:settings.optionMeta,theme:{background:'#252525',foreground:'#ededed',cursor:'#eeeeee',blue:'#409bff',green:'#83c969',selectionBackground:'#5c769b99'}});
  const addon=new FitAddon(),search=new SearchAddon();term.loadAddon(addon);term.loadAddon(search);term.open(container.current);terminal.current=term;fit.current=addon;renderer.current=createTerminalRenderer(term,()=>new WebglAddon());renderer.current.setActive(active);
  let loading=true,disposed=false;const syncSize=()=>{if(disposed||!container.current?.offsetWidth||!container.current?.offsetHeight)return;try{addon.fit();bridge.sshResize(hostId,term.cols,term.rows)}catch{}};const queued=[];const write=event=>{const data=bytes(event.data);term.write(data,()=>{if(!disposed)bridge.sshAck(hostId,data.length)})};
  const unsub=bridge.onSSHEvent(event=>{if(event.host!==hostId)return;if(event.type==='data'){if(loading)queued.push(event);else write(event)}else if(event.type==='state'&&event.state==='connected')requestAnimationFrame(syncSize);else if(event.type==='state'&&event.state==='disconnected')term.write('\r\n\x1b[90m[Connection closed]\x1b[0m\r\n')});
  bridge.sshSnapshot(hostId).then(snapshot=>{if(disposed)return;if(snapshot.state==='connected')requestAnimationFrame(syncSize);if(snapshot.data)term.write(bytes(snapshot.data),()=>{if(!disposed)bridge.sshAck(hostId,snapshot.pendingBytes||0)});loading=false;for(const event of queued)if(event.sequence>(snapshot.sequence||0))write(event);queued.length=0}).catch(e=>{if(!disposed)setError(e.message)});
  term.attachCustomKeyEventHandler(e=>{const action=options.current.bindings.find(b=>b[2]===chord(e))?.[1];return !action && !(bridge.platform==='darwin'?e.metaKey:e.ctrlKey&&e.shiftKey)&&!(e.key==='Escape'&&container.current?.closest('.session-screen')?.querySelector('.terminal-find'))});
  const edit=async action=>{try{await editTerminal(term,bridge,action);term.focus()}catch(e){setError(e.message)}};
  const unsubEdit=bridge.onTerminalEdit(r=>{if(r.hostId===hostId&&!disposed)edit(r.action)});
  const selection=term.onSelectionChange(()=>{if(options.current.settings.selectCopy&&term.hasSelection())editTerminal(term,bridge,'copy').catch(e=>setError(e.message))});
  const results=search.onDidChangeResults(r=>onResults(hostId,r));
  const api={focus:()=>term.focus(),insert:text=>{term.paste(text);term.focus()},edit,find:(text,previous=false)=>{if(!text){search.clearDecorations();onResults(hostId,{resultIndex:-1,resultCount:0});return}const ok=previous?search.findPrevious(text,{decorations:{matchBackground:'#71551f',matchBorder:'#c39e50',matchOverviewRuler:'#c39e50',activeMatchBackground:'#aa792e',activeMatchBorder:'#f5c468',activeMatchColorOverviewRuler:'#f5c468'}}):search.findNext(text,{decorations:{matchBackground:'#71551f',matchBorder:'#c39e50',matchOverviewRuler:'#c39e50',activeMatchBackground:'#aa792e',activeMatchBorder:'#f5c468',activeMatchColorOverviewRuler:'#f5c468'}});if(!ok)onResults(hostId,{resultIndex:-1,resultCount:0})}};register(hostId,api);
  const focus=()=>bridge.terminalFocus(hostId),blur=e=>{if(!container.current?.contains(e.relatedTarget))bridge.terminalFocus(null)};const el=container.current;el.addEventListener('focusin',focus);el.addEventListener('focusout',blur);
  const input=term.onData(data=>bridge.sshWrite(hostId,data)),resize=term.onResize(({cols,rows})=>bridge.sshResize(hostId,cols,rows));
  const observer=new ResizeObserver(()=>{if(el.offsetWidth&&el.offsetHeight){try{addon.fit()}catch{}}});observer.observe(el);
  return()=>{disposed=true;register(hostId,null);el.removeEventListener('focusin',focus);el.removeEventListener('focusout',blur);unsub();unsubEdit();selection.dispose();results.dispose();observer.disconnect();input.dispose();resize.dispose();renderer.current?.dispose();renderer.current=null;term.dispose();terminal.current=null};
 },[hostId]);
 useEffect(()=>{const t=terminal.current;if(!t)return;renderer.current?.setActive(active);t.options.fontFamily=terminalFontFamily(settings.font);t.options.fontSize=settings.fontSize;t.options.lineHeight=Number(settings.lineHeight);t.options.cursorBlink=settings.blink;t.options.cursorStyle=settings.cursor.toLowerCase();t.options.scrollback=settings.scrollback;t.options.macOptionIsMeta=Boolean(settings.optionMeta);if(active)requestAnimationFrame(()=>{try{fit.current.fit();window.holonetDesktop.sshResize(hostId,t.cols,t.rows);t.focus()}catch{}})},[active,settings]);
 return <div className={'native-terminal '+(active?'active':'')} aria-label={'SSH terminal '+hostId} onContextMenu={e=>{e.preventDefault();if(options.current.settings.rightPaste)editTerminal(terminal.current,window.holonetDesktop,'paste').catch(e=>setError(e.message));else window.holonetDesktop.terminalMenu({hostId,hasSelection:terminal.current?.hasSelection()===true})}}><div ref={container} className="terminal-mount"/>{error&&<div role="alert" className="inline-error">{error}</div>}</div>;
}
