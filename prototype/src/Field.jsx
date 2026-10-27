import React from 'react';
export function Field({label,hint,children}){const id=React.useId();return <div className="field"><label htmlFor={id}>{label}</label>{React.cloneElement(children,{id})}{hint&&<small>{hint}</small>}</div>}
