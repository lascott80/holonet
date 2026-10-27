import semver from 'semver';

export const RELEASES_API='https://api.github.com/repos/lascott80/holonet/releases?per_page=100';
const TOKEN_ID='github:update-token';
export const updateTokenStatus=async vault=>Boolean(await vault.get(TOKEN_ID));
export async function saveUpdateToken(vault,token){
 if(typeof token!=='string'||!token.trim()||token.length>4096||/[\r\n]/.test(token))throw Error('Enter a valid GitHub token.');
 await vault.put(TOKEN_ID,token.trim());
}
export const removeUpdateToken=vault=>vault.remove(TOKEN_ID);

// Only query the fixed GitHub API endpoint. Never log or return the token or
// accept release URLs supplied by the response.
export async function checkForUpdates({vault,currentVersion,platform,fetcher=fetch}){
 const token=await vault.get(TOKEN_ID);
 if(!semver.valid(currentVersion))throw Error('This application version cannot be compared.');
 let response;
 try{response=await fetcher(RELEASES_API,{headers:{Accept:'application/vnd.github+json',...(token?{Authorization:`Bearer ${token}`}:{ }),'X-GitHub-Api-Version':'2022-11-28','User-Agent':'Holonet-update-check'},redirect:'error',signal:AbortSignal.timeout(15000)})}catch{throw Error('Could not reach GitHub. Check your network and try again.')}
 if(response.status===401)throw Error('GitHub rejected the saved token. Replace or remove it in Settings > General > Updates.');
 if(response.status===403)throw Error('GitHub denied the request or its rate limit was reached. Try again later or add a GitHub token in Settings > General > Updates.');
 if(response.status===404)throw Error('The Holonet releases were not found on GitHub.');
 if(!response.ok)throw Error(`GitHub update check failed (HTTP ${response.status}).`);
 let releases;try{releases=await response.json()}catch{throw Error('GitHub returned an invalid release list.')}
 if(!Array.isArray(releases))throw Error('GitHub returned an invalid release list.');
 const beta=Boolean(semver.prerelease(currentVersion));
 const candidates=releases.filter(r=>r&&!r.draft&&typeof r.tag_name==='string'&&semver.valid(r.tag_name)&&(beta||(!r.prerelease&&!semver.prerelease(r.tag_name)))&&semver.gt(r.tag_name,currentVersion)).sort((a,b)=>semver.rcompare(a.tag_name,b.tag_name));
 if(!candidates.length)return {available:false,currentVersion};
 const release=candidates[0],assetPattern=platform==='darwin'?/^Holonet-.*-arm64\.dmg$/i:platform==='win32'?/^Holonet[ ._-]+Setup[ ._-]+.*\.exe$/i:null;
 const hasInstaller=Boolean(assetPattern&&Array.isArray(release.assets)&&release.assets.some(a=>a&&typeof a.name==='string'&&assetPattern.test(a.name)));
 return {available:true,currentVersion,version:semver.clean(release.tag_name),hasInstaller,url:'https://github.com/lascott80/holonet/releases/tag/'+encodeURIComponent(release.tag_name)};
}
