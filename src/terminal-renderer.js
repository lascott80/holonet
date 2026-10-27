// Keep just the visible terminal's GPU renderer allocated. Disposing the addon
// restores xterm's DOM renderer without replacing the terminal or its buffer.
export function createTerminalRenderer(term,createAddon,onRenderer=()=>{}){
 let addon=null,loss=null,failed=false,disposed=false;
 const release=()=>{
  const previous=addon;addon=null;
  loss?.dispose();loss=null;
  try{previous?.dispose()}finally{onRenderer('dom')}
 };
 return {
  setActive(active){
   if(disposed)return;
   if(!active){if(addon)release();return}
   if(addon||failed)return;
   try{
    addon=createAddon();
    loss=addon.onContextLoss(()=>{failed=true;release()});
    term.loadAddon(addon);
    onRenderer('webgl');
   }catch{
    failed=true;
    try{release()}catch{/* Keep the terminal usable if GPU initialization failed. */}
   }
  },
  dispose(){disposed=true;release()},
 };
}
