export const groupKey = g => g.id || [g.parent,g.name].filter(Boolean).join('/');
export function hostGroup(host,groups){return host.groupId || groupKey(groups.find(g=>g.name===(host.category||host.group)&&g.parent===(host.category?host.group:''))||{name:host.group,parent:''})}
export function effectiveHost(host,groups=[],identities=[]){
 const g=groups.find(g=>groupKey(g)===hostGroup(host,groups));
 const parent=g?.parentId?groups.find(x=>groupKey(x)===g.parentId):groups.find(x=>x.name===g?.parent&&!x.parent);
 const nonempty=o=>Object.fromEntries(Object.entries(o||{}).filter(([,v])=>v!==''&&v!==undefined));
 const defaults={...nonempty(parent?.defaults),...nonempty(g?.defaults)};
 const merged={...defaults,...Object.fromEntries(Object.entries(host).filter(([,v])=>v!==''&&v!==undefined))};
 const identity=identities.find(x=>x.id===merged.identity);
 return {...merged,...(identity?{user:merged.user||identity.user,auth:identity.auth,key:identity.key,credentialIdentity:identity.id}:{}),port:Number(merged.port||22),user:merged.user||identity?.user||'',auth:identity?.auth||merged.auth||'Password'};
}
// Removing organization must not remove hosts or change how they authenticate.
// Keep child groups, promoting them to the top level with their inherited defaults.
export function removeGroupFromState(state,id){
 const target=state.groups.find(g=>groupKey(g)===id);
 if(!target)return state;
 const nonempty=o=>Object.fromEntries(Object.entries(o||{}).filter(([,v])=>v!==''&&v!==undefined));
 const promoted=new Map();
 const rootNames=new Set(state.groups.filter(g=>groupKey(g)!==id&&!g.parent&&!g.parentId).map(g=>g.name));
 const groups=state.groups.filter(g=>groupKey(g)!==id).map(g=>{
  if(g.parentId!==id&&(g.parentId||g.parent!==target.name))return g;
  let name=g.name,index=1;
  while(rootNames.has(name))name=g.name+' ('+target.name+(index++>1?' '+(index-1):'')+')';
  rootNames.add(name);
  const next={...g,name,parent:'',parentId:'',defaults:{...nonempty(target.defaults),...nonempty(g.defaults)}};
  promoted.set(groupKey(g),next);return next;
 });
 const hosts=state.hosts.map(h=>{
  const owner=hostGroup(h,state.groups);
  if(owner===id){
   const resolved=effectiveHost(h,state.groups,state.identities);
   return {...h,groupId:'',group:'',category:'',user:resolved.user,port:resolved.port,auth:resolved.auth,key:resolved.key||'',identity:resolved.identity||''};
  }
  const child=promoted.get(owner);
  return child?{...h,group:child.name,category:''}:h;
 });
 return {...state,groups,hosts};
}
export function disconnectedSessionAction(session,settings,event){
 if(event.state!=='disconnected'||!session)return 'keep';
 const established=session.established||session.state==='connected';
 if(!established)return 'keep'; // Leave initial connection errors available to inspect.
 if(settings.dropped==='Close tab')return 'close';
 if(event.reason!=='shell-exit'&&settings.autoReconnect&&(session.retries||0)<3)return 'reconnect';
 return 'keep';
}
export function chord(event){return [event.metaKey?'⌘':null,event.ctrlKey?'⌃':null,event.shiftKey?'⇧':null,event.altKey?'⌥':null,event.key.length===1?event.key.toUpperCase():event.key].filter(Boolean).join(' ')}
export function accelerator(value){return value.split(' ').map(k=>({'⌘':'Command','⌃':'Control','⇧':'Shift','⌥':'Alt','=':'Plus','ArrowRight':'Right','ArrowLeft':'Left'}[k]||k)).join('+')}
export function parseConnection(value){
 const input=value.trim(),explicit=input.startsWith('ssh '),body=explicit?input.slice(4).trim():input;
 const match=/^(?:([^@\s]+)@)?(\[[0-9a-fA-F:]+\]|[a-zA-Z0-9._-]+)(?::(\d+))?(?:\s+-p\s+(\d+))?$/.exec(body);
 if(!match||(!explicit&&!match[1]&&!match[2].includes('.')&&!match[2].includes(':')))return null;
 const port=Number(match[4]||match[3]||22);if(port<1||port>65535)return null;
 return {name:match[2].replace(/^\[|\]$/g,''),address:match[2].replace(/^\[|\]$/g,''),user:match[1]||'',port,auth:'Password',icon:'server'};
}
