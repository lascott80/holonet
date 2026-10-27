import React,{useEffect,useRef,useState} from 'react';
export function PermissionsDialog({value,close,save}){
 const [mode,setMode]=useState(value.mode),[error,setError]=useState(''),[saving,setSaving]=useState(false),ref=useRef();
 useEffect(()=>{const previous=document.activeElement;ref.current?.querySelector('input')?.focus();return()=>previous?.isConnected&&previous.focus()},[]);
 const keys=e=>{if(e.key==='Escape'){e.stopPropagation();close();return}if(e.key!=='Tab')return;const controls=[...ref.current.querySelectorAll('input:not(:disabled),button:not(:disabled)')],first=controls[0],last=controls.at(-1);if(e.shiftKey&&document.activeElement===first){e.preventDefault();last.focus()}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first.focus()}};
 return <div className="overlay permissions-overlay" onMouseDown={e=>{if(e.target===e.currentTarget)close()}}><form ref={ref} className="dialog permissions-dialog" role="dialog" aria-modal="true" aria-labelledby="permissions-title" onKeyDown={keys} onSubmit={async e=>{e.preventDefault();setSaving(true);setError('');try{await save(mode)}catch(err){setError(err.message)}finally{setSaving(false)}}}>
  <h2 id="permissions-title">Edit permissions</h2><p className="permissions-path" title={value.path}>{value.path}</p>
  <div className="permissions-grid"><span></span>{['Read','Write','Execute'].map(label=><b key={label}>{label}</b>)}{[['Owner',6],['Group',3],['Others',0]].map(([label,shift])=><React.Fragment key={label}><b>{label}</b>{[['Read',4],['Write',2],['Execute',1]].map(([action,bit])=>{const mask=bit<<shift;return <label key={action}><input type="checkbox" aria-label={label+' '+action} checked={Boolean(mode&mask)} onChange={e=>setMode(current=>e.target.checked?current|mask:current&~mask)}/></label>})}</React.Fragment>)}</div>
  <p className="permissions-hint">Special permission bits are preserved.</p>{error&&<p className="inline-error" role="alert">{error}</p>}
  <footer><button type="button" className="secondary" onClick={close} disabled={saving}>Cancel</button><button type="submit" className="primary" disabled={saving}>{saving?'Saving…':'Save permissions'}</button></footer>
 </form></div>;
}
