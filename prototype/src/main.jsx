import React from 'react';
import {createRoot} from 'react-dom/client';
import {App} from './App.jsx';
import {loadState,flushState} from './persistence';
import './styles.css';
import {loadWithTerminalFont} from './terminal-font';
const root=createRoot(document.getElementById('root'));
loadWithTerminalFont(loadState,document.fonts).then(bootstrap=>{
  window.holonetDesktop?.onCloseRequested(flushState);
  root.render(<React.StrictMode><App bootstrap={bootstrap}/></React.StrictMode>);
}).catch(error=>{
  root.render(<main className="startup-error"><h1>Unable to load saved settings</h1><p>{error.message}</p><p>Your existing data has not been replaced.</p><button onClick={()=>window.location.reload()}>Try again</button></main>);
});
