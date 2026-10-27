import React,{useEffect,useState} from 'react';
import {Field} from './Field';
export function UpdateSettings(){
 const [saved,setSaved]=useState(false),[token,setToken]=useState(''),[busy,setBusy]=useState(false),[message,setMessage]=useState('');
 const bridge=window.holonetDesktop;
 useEffect(()=>{let active=true;bridge.updateTokenStatus().then(value=>{if(active)setSaved(value)},error=>{if(active)setMessage(error.message)});return()=>{active=false}},[bridge]);
 const store=async e=>{e.preventDefault();setBusy(true);setMessage('');try{await bridge.updateTokenSave(token);setToken('');setSaved(true);setMessage('GitHub token saved.')}catch(error){setMessage(error.message)}finally{setBusy(false)}};
 const remove=async()=>{setBusy(true);setMessage('');try{await bridge.updateTokenRemove();setSaved(false);setToken('');setMessage('GitHub token removed.')}catch(error){setMessage(error.message)}finally{setBusy(false)}};
 return <section className="updates-settings"><h3>Updates</h3><p className="section-help">Public releases can be checked without a token. Optionally add a GitHub token to increase API limits; it is stored in the encrypted credential vault and sent only to GitHub when checking for updates.</p><form onSubmit={store}><Field label="GitHub token"><input type="password" value={token} onChange={e=>setToken(e.target.value)} autoComplete="off" placeholder={saved?'Token saved; enter a new token to replace it':'Optional GitHub token'} maxLength={4096}/></Field><div className="updates-actions"><button className="secondary" type="submit" disabled={busy||!token.trim()}>{saved?'Replace token':'Save token'}</button>{saved&&<button type="button" onClick={remove} disabled={busy}>Remove token</button>}</div></form>{message&&<p role="status" className="section-help">{message}</p>}<p className="section-help">Use Help → Check for Updates. New releases open in your browser for manual installation.</p></section>;
}
