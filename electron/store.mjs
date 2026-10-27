import { mkdir, readFile, rename, open } from 'node:fs/promises';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { sanitizeState } from '../shared/state.mjs';

export async function atomicWrite(filename, contents) {
  const temporary = `${filename}.${randomUUID()}.tmp`;
  const file = await open(temporary, 'wx', 0o600);
  try { await file.writeFile(contents, 'utf8'); await file.sync(); } finally { await file.close(); }
  await rename(temporary, filename);
}

export class StateStore {
  #tail = Promise.resolve();
  constructor(directory, defaults) {
    this.directory = directory;
    this.filename = path.join(directory, 'state.json');
    this.defaults = sanitizeState(defaults);
  }
  async load() {
    try {
      const json = await readFile(this.filename, 'utf8');
      if (Buffer.byteLength(json) > 8 * 1024 * 1024) throw new Error('Settings file is too large');
      return sanitizeState(JSON.parse(json));
    } catch (error) {
      if (error.code === 'ENOENT') return structuredClone(this.defaults);
      throw new Error('Holonet could not read its saved settings. The original file has been left untouched.', {cause:error});
    }
  }
  save(input) {
    let state;
    try { state = sanitizeState(input); } catch (error) { return Promise.reject(error); }
    const contents = JSON.stringify(state, null, 2) + '\n';
    const write = this.#tail.catch(() => {}).then(async () => {
      await mkdir(this.directory, {recursive:true, mode:0o700});
      try {
        const previous = await readFile(this.filename, 'utf8');
        // Never replace an unreadable or future-version settings file.
        sanitizeState(JSON.parse(previous));
        await atomicWrite(this.filename + '.bak', previous);
      } catch (error) { if (error.code !== 'ENOENT') throw error; }
      await atomicWrite(this.filename, contents);
    });
    this.#tail = write;
    return write.then(() => ({saved:true}));
  }
  flush() { return this.#tail; }
}
