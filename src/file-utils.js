export const prettyBytes=n=>n<1024?n+' B':n<1024**2?(n/1024).toFixed(1)+' KB':n<1024**3?(n/1024**2).toFixed(1)+' MB':(n/1024**3).toFixed(1)+' GB';
