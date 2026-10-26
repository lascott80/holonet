export const initialHosts = [
 {id:'atlas',name:'atlas',address:'192.168.1.24',user:'lawrence',group:'Homelab',category:'Servers',icon:'server'},
 {id:'nas',name:'nas',address:'192.168.1.8',user:'lawrence',group:'Homelab',category:'Storage',icon:'storage'},
 {id:'orion',name:'orion',address:'192.168.1.33',user:'root',group:'Homelab',category:'Servers',icon:'server'},
 {id:'devbox',name:'devbox',address:'192.168.1.42',user:'lawrence',group:'Homelab',category:'Servers',icon:'computer'},
 {id:'gateway',name:'gateway',address:'192.168.1.1',user:'lawrence',group:'Homelab',category:'Networking',icon:'network'},
 {id:'cloud-web',name:'cloud-web',address:'157.245.12.77',user:'ubuntu',group:'Cloud',category:'DigitalOcean',icon:'cloud'},
];
export const initialGroups=[{name:'Homelab',parent:'',icon:'folder'},{name:'Servers',parent:'Homelab',icon:'server'},{name:'Networking',parent:'Homelab',icon:'network'},{name:'Storage',parent:'Homelab',icon:'storage'},{name:'Cloud',parent:'',icon:'cloud'},{name:'AWS',parent:'Cloud',icon:'cloud'},{name:'DigitalOcean',parent:'Cloud',icon:'cloud'},{name:'Cloudflare',parent:'Cloud',icon:'cloud'}];
export const initialKeys=[{id:'personal',name:'Personal Mac',type:'ED25519',comment:'lawrence@macbook'},{id:'homelab',name:'Homelab',type:'ED25519',comment:'homelab'},{id:'cloud',name:'Cloud access',type:'RSA 4096',comment:'cloud'}];
export const initialSettings={font:'JetBrains Mono',fontSize:15,lineHeight:'1.4',cursor:'Block',blink:true,scrollback:10000,launch:'Hosts',reopen:true,login:false,confirmClose:true,dropped:'Keep tab open',window:true,confirmQuit:true,local:'~/Downloads',remote:'Home directory',hidden:false,folders:true,conflicts:'Ask each time',concurrent:2,queue:'When transfers are active'};
export const fileSets={local:[['assets','—',true],['scripts','—',true],['README.md','3.2 KB'],['compose.yml','1.4 KB'],['backup.tar.gz','24.8 MB'],['.env.example','820 B']],remote:[['backups','—',true],['config','—',true],['README.md','2.9 KB'],['compose.yml','1.2 KB'],['app.log','18.6 KB']]};
export const defaultBindings=[['Sessions','New session','⌘ T'],['Sessions','Close session','⌘ W'],['Sessions','Next session','⌃ Tab'],['Sessions','Previous session','⌃ ⇧ Tab'],['Terminal','Find in terminal','⌘ F'],['Terminal','Copy selection','⌘ C'],['Terminal','Paste','⌘ V'],['Terminal','Increase font size','⌘ ='],['Files','Toggle Terminal / SFTP','⌘ ⇧ E'],['Terminal','Select all','⌘ A'],['Sessions','Command palette','⌘ K'],['Terminal','Terminal tools','⌘ .']];
export const fingerprint='SHA256:Q7m4Lx2pN8vR5kT9sW3hB6cD1fG0jY4aE2uI8oP5zXc';

export const bindingsForPlatform=platform=>defaultBindings.map(([group,action,chord])=>[group,action,platform==='darwin'?chord:chord.replace('⌘',['Copy selection','Paste','Select all'].includes(action)?'⌃ ⇧':'⌃')]);
