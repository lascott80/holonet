// xterm caches character-cell measurements when opened. Load the bundled face
// first so a late font swap cannot leave fallback-font widths behind.
export const terminalFontFamily=font=>font==='JetBrains Mono'?'"JetBrains Mono", monospace':'monospace';
export async function loadWithTerminalFont(load,fonts){
 const [bootstrap]=await Promise.all([
  load(),
  Promise.resolve().then(()=>fonts?.load('15px "JetBrains Mono"','MW0123456789')).catch(()=>undefined),
 ]);
 return bootstrap;
}
