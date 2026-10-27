import builder from 'electron-builder';
import fs from 'node:fs/promises';
const {build,Platform,Arch}=builder;
const pkg=JSON.parse(await fs.readFile(new URL('../package.json',import.meta.url),'utf8'));
// ssh2's optional native acceleration is not required. Exclude host-platform
// addons for cross-builds; Windows uses its built-in Node crypto implementation.
await build({targets:Platform.WINDOWS.createTarget(['nsis'],Arch.arm64),config:{npmRebuild:false,files:[...pkg.build.files,'!node_modules/**/*.node'],win:{signAndEditExecutable:false},directories:{output:'release/windows-arm64'}}});
