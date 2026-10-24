#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// 浏览器仅用于离线宣传素材，不进入应用包。
import {spawn, spawnSync} from 'node:child_process';
import {mkdir, readFile, writeFile, mkdtemp, rm, copyFile} from 'node:fs/promises';
import {resolve, join} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL} from 'node:url';
import {once} from 'node:events';

const args = process.argv.slice(2);
const options = {};
for (let index = 0; index < args.length; index += 2) {
  if (!['--mermaid-js', '--browser', '--output'].includes(args[index]) || !args[index + 1]) {
    throw new Error('Usage: tools/make-release-comparison.mjs --mermaid-js FILE [--browser FILE] [--output DIR]');
  }
  options[args[index]] = args[index + 1];
}
if (!options['--mermaid-js']) throw new Error('--mermaid-js is required (use an existing local Mermaid bundle)');
const output = resolve(options['--output'] ?? 'target/promo');
await mkdir(output, {recursive: true});
const markdown = await readFile('fixtures/demo-vault/Learning/Learning map.md', 'utf8');
const source = markdown.match(/```mermaid\n([\s\S]*?)```/)[1];
const mermaid = await readFile(options['--mermaid-js'], 'utf8');
const rendered = spawnSync(resolve('target/release/hortus'), ['mermaid', 'render',
  'fixtures/demo-vault/Learning/Learning map.md', '--width', '700',
  '--out', join(output, 'hortus-700.svg'), '--metrics', join(output, 'hortus-700.json')], {encoding:'utf8'});
if (rendered.error) throw rendered.error;
if (rendered.status !== 0) throw new Error(rendered.stderr || 'Build target/release/hortus before generating the comparison');
const profile = await mkdtemp(join(tmpdir(), 'hortus-promo-browser-'));
const browser = spawn(options['--browser'] ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
  '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
  '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
  '--disable-component-update', '--disable-default-apps', '--disable-sync',
], {stdio: ['ignore', 'ignore', 'pipe']});
let socket;
let sequence = 0;
const pending = new Map();
try {
  const endpoint = await new Promise((accept, reject) => {
    let log = '';
    const timeout = setTimeout(() => reject(new Error('Browser endpoint timed out')), 15000);
    browser.stderr.on('data', chunk => {
      log += chunk.toString();
      const match = log.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (match) {clearTimeout(timeout); accept(match[1]);}
    });
    browser.once('error', reject);
    browser.once('exit', code => reject(new Error(`Browser exited: ${code}`)));
  });
  socket = new WebSocket(endpoint);
  await once(socket, 'open');
  socket.addEventListener('message', event => {
    const data = JSON.parse(event.data);
    if (!data.id) return;
    const callback = pending.get(data.id);
    pending.delete(data.id);
    if (data.error) callback.reject(new Error(JSON.stringify(data.error)));
    else callback.accept(data.result);
  });
  const send = (method, params = {}, sessionId) => new Promise((accept, reject) => {
    const id = ++sequence;
    pending.set(id, {accept, reject});
    socket.send(JSON.stringify({id, method, params, ...(sessionId ? {sessionId} : {})}));
  });
  const {targetId} = await send('Target.createTarget', {url: 'about:blank'});
  const {sessionId} = await send('Target.attachToTarget', {targetId, flatten: true});
  const evaluate = async expression => {
    const result = await send('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true}, sessionId);
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await send('Page.enable', {}, sessionId);
  await send('Emulation.setDeviceMetricsOverride', {width: 700, height: 1100, deviceScaleFactor: 2, mobile: false}, sessionId);
  await send('Page.setDocumentContent', {frameId: (await send('Page.getFrameTree', {}, sessionId)).frameTree.frame.id,
    html: '<html><body style="margin:0;background:white"><main style="width:700px" id="diagram"></main></body></html>'}, sessionId);
  await evaluate(mermaid);
  await evaluate(`(async () => {mermaid.initialize({startOnLoad:false, securityLevel:'strict'}); const result=await mermaid.render('learning-map', ${JSON.stringify(source)}); document.getElementById('diagram').innerHTML=result.svg; await document.fonts.ready; return true;})()`);
  const baseline = await evaluate(`(() => {const svg=document.querySelector('svg'); return {svg:svg.outerHTML, viewBox:svg.getAttribute('viewBox'), bounds:{width:svg.getBoundingClientRect().width,height:svg.getBoundingClientRect().height}, nodes:svg.querySelectorAll('.node').length};})()`);
  await writeFile(join(output, 'mermaid-default.svg'), baseline.svg);
  await writeFile(join(output, 'mermaid-default.json'), JSON.stringify({...baseline, svg: undefined}, null, 2) + '\n');
  const capture = async (name, width, height) => {
    const {data} = await send('Page.captureScreenshot', {format:'png', clip:{x:0,y:0,width,height,scale:1}, captureBeyondViewport:true}, sessionId);
    await writeFile(join(output, name), Buffer.from(data, 'base64'));
  };
  await capture('mermaid-default-700.png', 700, Math.ceil(baseline.bounds.height));
  const native = await readFile(join(output, 'hortus-700.svg'), 'utf8');
  const nativeHeight = Number(native.match(/<svg[^>]*height="([\d.]+)"/)[1]);
  const imageData = value => 'data:image/svg+xml;base64,' + Buffer.from(value).toString('base64');
  const comparison = `<!doctype html><html><head><meta charset="utf-8"><style>
    *{box-sizing:border-box}body{margin:0;background:#f6f7f8;color:#252a2d;font-family:-apple-system,BlinkMacSystemFont,Arial,sans-serif}
    header{padding:30px 36px 22px}h1{font-size:28px;font-weight:650;margin:0 0 8px}p{margin:0;color:#626b70;font-size:16px}
    main{display:grid;grid-template-columns:700px 700px;gap:28px;margin:0 36px}section{background:white}
    h2{font-size:19px;margin:0;padding:18px 20px 6px;font-weight:600}section p{font-size:14px;padding:0 20px 18px}
    img{display:block;width:700px;height:auto}footer{font-size:14px;color:#626b70;padding:20px 36px}
  </style></head><body><header><h1>The same 81-node Mermaid diagram, at 700 px</h1><p>When a tree gets too wide, Hortus changes the layout so the text stays readable.</p></header><main>
    <section><h2>Mermaid default</h2><p>Default flowchart layout, fitted to the note width</p><img src="${imageData(baseline.svg)}"></section>
    <section><h2>Hortus</h2><p>Responsive tree layout, rendered by the native Rust core</p><img src="${imageData(native)}"></section>
  </main><footer>Same source · 81 nodes · 700 px per diagram · Original CC0 demo notebook</footer></body></html>`;
  const html = join(output, 'comparison.html');
  await writeFile(html, comparison);
  await send('Page.navigate', {url:pathToFileURL(html).href}, sessionId);
  for (let attempt=0; attempt<100; attempt++) {
    if (await evaluate(`document.readyState === 'complete' && document.images.length === 2 && [...document.images].every(i=>i.complete && i.naturalWidth>0)`)) break;
    await new Promise(accept => setTimeout(accept, 50));
    if (attempt === 99) throw new Error('Comparison images failed to load');
  }
  await send('Emulation.setDeviceMetricsOverride', {width:1500,height:Math.ceil(nativeHeight)+250,deviceScaleFactor:1,mobile:false}, sessionId);
  await capture('mermaid-comparison.png', 1500, await evaluate('document.documentElement.scrollHeight'));
  await mkdir('docs/images', {recursive:true});
  await copyFile(join(output,'mermaid-comparison.png'), 'docs/images/mermaid-comparison.png');
  console.log(JSON.stringify({output,baselineNodes:baseline.nodes,baselineViewBox:baseline.viewBox,nativeHeight}));
  await send('Browser.close');
} finally {
  socket?.close();
  if (browser.exitCode === null && browser.signalCode === null) {
    const stopped = once(browser, 'exit');
    browser.kill('SIGTERM');
    await stopped;
  }
  await rm(profile,{recursive:true,force:true});
}
