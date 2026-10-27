// Measures the built app in isolated Electron instances; never touches the user's profile.
import {_electron as electron} from 'playwright-core';
import executablePath from 'electron';
import {mkdtemp,mkdir,writeFile,rm} from 'node:fs/promises';
import {performance} from 'node:perf_hooks';
import os from 'node:os';import path from 'node:path';
import {initialSettings,initialGroups,defaultBindings} from '../src/data.js';
const runs=Number(process.env.HOLONET_BENCH_RUNS||3);
const median=values=>[...values].sort((a,b)=>a-b)[Math.floor(values.length/2)];
const rows=[];
for(const tabs of [0,10])for(let run=0;run<runs;run++){
 const dir=await mkdtemp(path.join(os.tmpdir(),'holonet-renderer-'));let app;
 try{
  await mkdir(path.join(dir,'holonet'),{recursive:true});
  const host={id:'fixture',name:'Benchmark host',address:'127.0.0.1',user:'test',auth:'Password'};
  const state={version:1,hosts:[host],groups:initialGroups,keyMetadata:[],settings:{...initialSettings,rememberTabs:true,confirmQuit:false},bindings:defaultBindings,sessionTabs:Array.from({length:tabs},(_,i)=>({id:`tab-${i}`,host:host.id,mode:'terminal'}))};
  await writeFile(path.join(dir,'holonet','state.json'),JSON.stringify(state));
  const start=performance.now();app=await electron.launch({executablePath,args:['.'],env:{...process.env,HOLONET_USER_DATA:dir},timeout:30000});
  const page=await app.firstWindow();await page.getByRole('heading',{name:'Hosts',exact:true}).first().waitFor();if(tabs)await page.waitForFunction(expected=>document.querySelectorAll('.xterm').length===expected,tabs);
  const startup=performance.now()-start;
  const metrics=await page.evaluate(()=>({navigationToReady:performance.now(),heapBytes:performance.memory?.usedJSHeapSize??null}));
  const renderer=await app.evaluate(({app,BrowserWindow})=>{const pid=BrowserWindow.getAllWindows()[0].webContents.getOSProcessId();return app.getAppMetrics().find(p=>p.pid===pid)?.memory||null});
  rows.push({tabs,startup,renderer,heapBytes:metrics.heapBytes,navigationToReady:metrics.navigationToReady});
 }finally{if(app)await app.close().catch(()=>{});await rm(dir,{recursive:true,force:true})}
}
for(const tabs of [0,10]){const samples=rows.filter(x=>x.tabs===tabs);const memory=samples.map(x=>x.renderer?.workingSetSize).filter(Number.isFinite);const heap=samples.map(x=>x.heapBytes).filter(Number.isFinite);console.log(`${tabs} restored tabs (${samples.length} runs): launch to usable median ${median(samples.map(x=>x.startup)).toFixed(1)} ms; renderer navigation to usable median ${median(samples.map(x=>x.navigationToReady)).toFixed(1)} ms; renderer working set ${memory.length?(median(memory)/1024).toFixed(1)+' MiB':'unavailable'}; JS heap ${heap.length?(median(heap)/1048576).toFixed(1)+' MiB':'unavailable'}`)}
