// SPDX-License-Identifier: MIT OR Apache-2.0
// Disposable Obsidian runtime comparison; only the owned process is terminated.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import {spawn} from 'node:child_process';
import {parseArgs} from 'node:util';
const {values}=parseArgs({options:{fixture:{type:'string'},output:{type:'string',default:'target/reports/obsidian'},app:{type:'string',default:'/Applications/Obsidian.app'},port:{type:'string',default:'19347'}}});
if(!values.fixture)throw Error('--fixture public-boundaries.md required');
const output=path.resolve(values.output);
await fs.mkdir(path.join(output,'raw'),{recursive:true});
await fs.mkdir(path.join(output,'render'),{recursive:true});
const port=Number(values.port), endpoint=`http://127.0.0.1:${port}`;
try { await fetch(endpoint+'/json/version'); throw Error('CDP port already in use; choose --port'); } catch(error) { if(!error.cause) throw error; }
const state=await fs.mkdtemp(path.join(os.tmpdir(),'math-spike-obsidian-'));
const vault=path.join(state,'Math spike'); const profile=path.join(state,'profile');
await fs.mkdir(path.join(vault,'.obsidian'),{recursive:true}); await fs.mkdir(profile);
const fixture=await fs.readFile(values.fixture,'utf8');
await fs.writeFile(path.join(vault,'Math boundary fixture.md'),fixture);
await fs.writeFile(path.join(vault,'.obsidian','app.json'),JSON.stringify({defaultViewMode:'preview',readableLineLength:false,showInlineTitle:false}));
await fs.writeFile(path.join(vault,'.obsidian','appearance.json'),JSON.stringify({theme:'moonstone',baseFontSize:15}));
const id=crypto.createHash('sha256').update(vault).digest('hex').slice(0,16);
await fs.writeFile(path.join(profile,'obsidian.json'),JSON.stringify({vaults:{[id]:{path:vault,ts:Date.now(),open:true}}}));
const log=await fs.open(path.join(output,'raw','obsidian-launch.log'),'w');
const proc=spawn(path.join(values.app,'Contents/MacOS/Obsidian'),[`--user-data-dir=${profile}`,`--remote-debugging-port=${port}`,'--remote-debugging-address=127.0.0.1',`obsidian://open?vault=${id}&file=Math%20boundary%20fixture`],{stdio:['ignore',log.fd,log.fd]});
proc.on('error',error=>{console.error(error.message)});
const delay=ms=>new Promise(r=>setTimeout(r,ms)); let ws;
try {
 let targets;
 for(let i=0;i<40;i++){try{targets=await(await fetch(endpoint+'/json/list')).json();if(targets.some(t=>t.type==='page'&&t.url.startsWith('app://')))break;}catch{} await delay(500);}
 const t=targets?.find(t=>t.type==='page'&&t.url.startsWith('app://')); if(!t)throw Error('no Obsidian page');
 ws=new WebSocket(t.webSocketDebuggerUrl); await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});
 let seq=0; const waits=new Map();
 ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const p=waits.get(m.id);waits.delete(m.id);clearTimeout(p.timer);m.error?p.reject(m.error):p.resolve(m.result)}};
 const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;const timer=setTimeout(()=>{waits.delete(id);reject(Error('CDP command timed out: '+method))},10000);waits.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}))});
 const evaluate=async expression=>{const r=await send('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value};
 await send('Runtime.enable'); await send('Page.enable');
 await send('Emulation.setDeviceMetricsOverride',{width:1300,height:4800,deviceScaleFactor:1,mobile:false});
 for(let i=0;i<40;i++){if(await evaluate('typeof app!=="undefined" && !!app.workspace && !!app.vault'))break;await delay(500);}
 await evaluate('(async()=>{await new Promise(resolve=>app.workspace.onLayoutReady(resolve));return true})()');
 await evaluate(`(async()=>{const f=app.vault.getAbstractFileByPath('Math boundary fixture.md'); const leaf=app.workspace.getLeaf(false); await leaf.openFile(f);await leaf.setViewState({type:'markdown',state:{file:f.path,mode:'preview'}});return true})()`);
 await delay(3500);
 await evaluate(`(()=>{const e=document.querySelector('.markdown-preview-view');e.scrollTop=e.scrollHeight;return e.scrollHeight})()`);
 await delay(1200);
 await evaluate(`(()=>{document.querySelector('.markdown-preview-view').scrollTop=0;return true})()`);
 await delay(1000);
 const dom=await evaluate(`(()=>{const root=document.querySelector('.markdown-preview-view');const headings=[...root.querySelectorAll('h2')];return {title:document.title,version:app.getVersion?.(),mode:app.workspace.getActiveViewOfType?.(Object)?.getMode?.(),html:root.innerHTML,sections:headings.map(h=>{const section=h.parentElement;let parts=[section.outerHTML];let n=section.nextElementSibling;while(n&&!n.querySelector('h2')){parts.push(n.outerHTML);n=n.nextElementSibling}const holder=document.createElement('div');holder.innerHTML=parts.join('');return {title:h.textContent,text:holder.textContent,math:[...holder.querySelectorAll('mjx-container')].map(x=>({display:x.getAttribute('display'),text:x.textContent,outer:x.outerHTML}))}}),mathCount:root.querySelectorAll('mjx-container').length}})()`);
 const environment=await send('Browser.getVersion');
 await fs.writeFile(path.join(output,'obsidian-environment.json'),JSON.stringify(environment,null,2));
 await fs.writeFile(path.join(output,'obsidian-dom.json'),JSON.stringify(dom,null,2));
 const screenshot=await send('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await fs.writeFile(path.join(output,'render','obsidian-boundaries.png'),Buffer.from(screenshot.data,'base64'));
 console.log(JSON.stringify({title:dom.title,sections:dom.sections.map(x=>({title:x.title,math:x.math.length})),mathCount:dom.mathCount},null,2));
}finally{if(ws)ws.close();proc.kill('SIGTERM');await delay(1000);await log.close();await fs.rm(state,{recursive:true,force:true});}
