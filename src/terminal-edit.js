// Clipboard operations use xterm's selection and paste pipeline, never DOM selection
// or a direct SSH write (which would bypass bracketed-paste handling).
export async function editTerminal(term, bridge, action) {
  if (action === 'copy') {
    const text = term.getSelection();
    if (text) await bridge.clipboardWrite(text);
  } else if (action === 'paste') {
    const text = await bridge.clipboardRead();
    if (text) term.paste(text);
  } else if (action === 'selectAll') term.selectAll();
  else if (action === 'clear') term.clear();
}

export function clipboardShortcut(event, platform) {
  if (event.altKey) return false;
  const key = event.key.toLowerCase();
  return ['c', 'v', 'a'].includes(key) &&
    (platform === 'darwin' ? event.metaKey && !event.ctrlKey : event.ctrlKey && event.shiftKey && !event.metaKey);
}
