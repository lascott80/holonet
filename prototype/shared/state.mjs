export const STATE_VERSION = 1;
const icons = new Set(['server', 'storage', 'computer', 'network', 'cloud', 'folder', 'code']);
const text = (value, name, max = 4096) => {
  if (typeof value !== 'string' || value.length > max || value.includes('\0')) throw new Error(`Invalid ${name}`);
  return value;
};
const list = (value, name, limit = 10000) => {
  if (!Array.isArray(value) || value.length > limit) throw new Error(`Invalid ${name}`);
  return value;
};
const object = (value, name) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Invalid ${name}`);
  return value;
};
const number = (value, name, min, max) => {
  if (!Number.isFinite(value) || value < min || value > max) throw new Error(`Invalid ${name}`);
  return value;
};
const optional = (result, source, name, max) => { if (source[name] != null) result[name] = text(source[name], name, max); };
const enumSetting = (value, name, options) => {
  if (!options.includes(value)) throw new Error(`Invalid ${name}`);
  return value;
};
export function sanitizeState(input) {
  const source = object(input, 'state');
  if (source.version !== STATE_VERSION) throw new Error('Unsupported settings version');
  const hosts = list(source.hosts, 'hosts').map(value => {
    const host = object(value, 'host');
    const result = {};
    for (const name of ['id', 'name', 'address', 'user', 'group', 'category']) result[name] = text(host[name] ?? '', name, 512);
    if (!result.id || !result.name || !result.address) throw new Error('Host identity is incomplete');
    result.icon = icons.has(host.icon) ? host.icon : 'server';
    result.port = host.port===''?'':number(Number(host.port ?? 22), 'port', 1, 65535);
    if (result.port!=='' && !Number.isInteger(result.port)) throw new Error('Invalid port');
    for (const name of ['auth', 'key', 'jump', 'startup', 'localFolder', 'remoteFolder','groupId','identity','os']) optional(result, host, name, 4096);
    if (host.keepalive != null) result.keepalive = number(Number(host.keepalive), 'keepalive', 0, 86400);
    if(host.tags)result.tags=list(host.tags,'tags',50).map(v=>text(v,'tag',80));
    if(host.createdAt)result.createdAt=number(host.createdAt,'created date',0,1e16);
    return result;
  });
  if (new Set(hosts.map(h => h.id)).size !== hosts.length) throw new Error('Duplicate host IDs');
  const groups = list(source.groups, 'groups', 1000).map(value => {
    const group = object(value, 'group');
    const result={ name: text(group.name, 'group name', 512), parent: text(group.parent ?? '', 'parent', 512), icon: icons.has(group.icon) ? group.icon : 'folder' };
    for(const key of ['id','parentId'])optional(result,group,key,512);
    if(group.defaults){result.defaults={};for(const key of ['user','port','identity','auth','key'])if(group.defaults[key]!=null)result.defaults[key]=text(String(group.defaults[key]),key,512)}
    return result;
  });
  if (groups.some(g => !g.name || g.name === g.parent)) throw new Error('Invalid group hierarchy');
  if (new Set(groups.map(g => JSON.stringify([g.parent, g.name]))).size !== groups.length) throw new Error('Duplicate groups');
  if (groups.some(g => g.parent && !groups.some(parent => parent.name === g.parent && !parent.parent))) throw new Error('Unknown parent group');
  const keyMetadata = list(source.keyMetadata ?? [], 'key metadata').map(value => {
    const key = object(value, 'key metadata');
    const metadata = { id: text(key.id, 'key id', 512), name: text(key.name, 'key name', 512), type: text(key.type, 'key type', 64), comment: text(key.comment ?? '', 'key comment', 4096) };
    optional(metadata,key,'fingerprint',256);optional(metadata,key,'publicKey',16384);return metadata;
  });
  const settings = object(source.settings, 'settings');
  const cleanSettings = {};
  const enums = {font:['JetBrains Mono','monospace'], lineHeight:['1.0','1.2','1.4','1.6','1.8'], cursor:['Block','Bar','Underline'], launch:['Hosts','Last visited screen'], dropped:['Keep tab open','Close tab'], remote:['Home directory','Last visited folder'], conflicts:['Ask each time','Replace existing'], queue:['When transfers are active','Always']};
  for (const [name, options] of Object.entries(enums)) cleanSettings[name] = enumSetting(settings[name], name, options);
  for (const name of ['blink','reopen','login','confirmClose','window','confirmQuit','hidden','folders']) {
    if (typeof settings[name] !== 'boolean') throw new Error(`Invalid ${name}`);
    cleanSettings[name] = settings[name];
  }
  for (const [name,min,max] of [['fontSize',10,28],['scrollback',100,100000],['concurrent',1,8]]) cleanSettings[name] = number(settings[name],name,min,max);
  cleanSettings.local = text(settings.local, 'local folder', 4096);
  for(const name of ['selectCopy','rightPaste','optionMeta','autoReconnect','showGroupedHosts','rememberTabs'])if(settings[name]!=null){if(typeof settings[name]!=='boolean')throw Error('Invalid '+name);cleanSettings[name]=settings[name]}
  const bindings = list(source.bindings, 'bindings', 100).map(binding => {
    if (!Array.isArray(binding) || binding.length !== 3) throw new Error('Invalid shortcut');
    return binding.map(value => text(value, 'shortcut', 120));
  });
  const knownHosts = new Set(hosts.map(h => h.id));
  const sessionTabs = list(source.sessionTabs ?? [], 'session tabs').map(tab => ({host:text(tab.host,'tab host',512),...(tab.id?{id:text(tab.id,'session ID',512)}:{}), mode:enumSetting(tab.mode,'tab mode',['terminal','sftp'])})).filter(tab => knownHosts.has(tab.host));
  const extra={};
  if(source.identities)extra.identities=list(source.identities,'identities',1000).map(i=>({id:text(i.id,'identity id',512),name:text(i.name,'identity name',512),user:text(i.user||'','identity user',512),auth:enumSetting(i.auth,'auth',['Password','SSH key']),key:text(i.key||'','identity key',512)}));
  if(source.snippets)extra.snippets=list(source.snippets,'snippets',1000).map(i=>({id:text(i.id,'snippet id',512),name:text(i.name,'snippet name',512),package:text(i.package||'','package',512),script:text(i.script||'','script',65536)}));
  if(source.forwardings)extra.forwardings=list(source.forwardings,'forwardings',1000).map(i=>({id:text(i.id,'rule id',512),name:text(i.name,'rule name',512),host:text(i.host,'rule host',512),type:enumSetting(i.type,'forward type',['local','remote','dynamic']),bind:text(i.bind||'127.0.0.1','bind address',512),port:number(Number(i.port),'listen port',1,65535),destination:text(i.destination||'127.0.0.1','destination',512),destinationPort:number(Number(i.destinationPort||80),'destination port',1,65535)}));
  return {version:STATE_VERSION,hosts,groups,keyMetadata,settings:cleanSettings,bindings,sessionTabs:[...new Map(sessionTabs.map(tab=>[tab.id||tab.host,tab])).values()],...extra};
}
