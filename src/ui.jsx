import React from 'react';
import {HardDrives,Folder,Desktop,Globe,Cloud,Code} from '@phosphor-icons/react';
export const icons={server:HardDrives,storage:HardDrives,computer:Desktop,network:Globe,cloud:Cloud,folder:Folder,code:Code};
export function HostIcon({host}){const Icon=icons[host.icon]||HardDrives;return <span className={'host-icon '+(host.icon||'server')}><Icon size={23} weight="regular"/></span>}
export function IconButton({label,children,...props}){return <button className="icon-button" title={label} aria-label={label} {...props}>{children}</button>}
