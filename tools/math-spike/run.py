#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Collect independent math-spike evidence. Build separately; benchmark serially."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import subprocess

ROOT = Path(__file__).resolve().parent

def run(*args, **kw):
    result = subprocess.run([str(x) for x in args], text=True, capture_output=True, **kw)
    if result.returncode:
        raise RuntimeError(f'{args}: {result.stdout}\n{result.stderr}')
    return result.stdout

def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')

def stats(values):
    values = sorted(values)
    n = len(values)
    return {'count': n, 'median_ns': values[n//2], 'p95_ns': values[math.ceil(n*.95)-1], 'max_ns': values[-1]}

def metadata(manifest):
    return json.loads(run('cargo', 'metadata', '--locked', '--offline', '--format-version', '1', '--manifest-path', manifest))

def collect(args, out):
    binary = ROOT/'target/release/math-spike'
    run(binary, 'samples', args.samples, out/'results.jsonl')
    rows = [json.loads(x) for x in (out/'results.jsonl').read_text().splitlines()]
    groups = defaultdict(lambda: Counter(total=0, parse_ok=0, validate_ok=0, panic=0))
    item_types = Counter()
    for row in rows:
        c = groups[row['style']+':'+row['group']]
        c.update(total=1, parse_ok=int(row['parse_ok'] is True), validate_ok=int(row['validate_ok']), panic=int(row['panic']))
        item_types.update(x['type'] for x in row.get('display', {}).get('items', []))
    write(out/'coverage.json', {'groups':dict(groups), 'drawing_items':dict(item_types)})
    if args.markdown:
        run(binary, 'markdown', args.markdown, out/'markdown-events.json')
    meta = metadata(ROOT/'Cargo.toml')
    # Include all resolved packages, including target-specific branches.
    packages = [{k:p.get(k) for k in ['name','version','license','license_file','source']} for p in meta['packages']]
    write(out/'dependencies.json', sorted(packages, key=lambda p:p['name']))
    (out/'cargo-tree.txt').write_text(run('cargo','tree','--locked','--offline','--target','all','--manifest-path',ROOT/'Cargo.toml'))
    write(out/'binary-size.json', {name:(ROOT/'target/release'/name).stat().st_size for name in ['size-empty','size-ratex','math-spike']})
    write(out/'machine.json', {'platform':platform.platform(), 'architecture':platform.machine(), 'rust':run('rustc','-Vv'), 'macos':run('sw_vers'), 'cpu':run('sysctl','-n','machdep.cpu.brand_string').strip(), 'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()})

def bench(args, out):
    binary = ROOT/'target/release/math-spike'
    # First-call times exclude process spawn, JSON encoding and IO, and include lazy init.
    rows = [json.loads(x) for x in (out/'results.jsonl').read_text().splitlines()]
    cold = []
    for _ in range(3):
        for row in rows:
            data = json.loads(run(binary, 'once', row['style'], row['input']))
            cold.append({'id':row['id'], 'style':row['style'], 'elapsed_ns':data['elapsed_ns'], 'ok':data['validate_ok'], 'panic':data['panic']})
    write(out/'first-call-raw.json',cold)
    write(out/'first-call.json', {mode:{'all':stats([x['elapsed_ns'] for x in cold if x['style']==mode]), 'success':stats([x['elapsed_ns'] for x in cold if x['style']==mode and x['ok']])} for mode in ['display','text']})
    run(binary,'bench',args.samples,out/'performance.json')

def render(args, out):
    meta = metadata(ROOT/'reference/Cargo.toml')
    write(out/'reference-dependencies.json', sorted([{k:p.get(k) for k in ['name','version','license','license_file','source']} for p in meta['packages']], key=lambda p:p['name']))
    (out/'reference-cargo-tree.txt').write_text(run('cargo','tree','--locked','--offline','--target','all','--manifest-path',ROOT/'reference/Cargo.toml'))
    pkg = next(p for p in meta['packages'] if p['name']=='ratex-katex-fonts')
    fonts = Path(pkg['manifest_path']).parent/'fonts'
    font_rows = [{'name':p.name,'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(fonts.glob('*.ttf'))]
    write(out/'fonts.json',font_rows)
    shutil.copyfile(fonts/'OFL.txt',out/'KaTeX-OFL.txt')
    swift = ROOT/'target/math-render'
    run('swiftc','-warnings-as-errors',ROOT/'render/main.swift','-o',swift)
    rows = [json.loads(x) for x in (out/'results.jsonl').read_text().splitlines()]
    # Twenty cases, including all four drawing item variants and CJK.
    ids = [3,9,14,19,29,34,51,69,72,73,78,80,89,107,108,110,118,122,126,138]
    selected = [x for x in rows if x['id'] in ids and x['style']=='display' and x['validate_ok']]
    assert len(selected)==20, [(x['id'],x['input']) for x in selected]
    render_dir = out/'render'; render_dir.mkdir(exist_ok=True)
    write(render_dir/'index.json', [{'id':x['id'],'input':x['input']} for x in selected])
    for row in selected:
        inp = render_dir/f'{row["id"]:03}.json'; write(inp,row)
        for theme in ['light','dark']:
            run(swift,'formula',fonts,inp,render_dir/f'{row["id"]:03}-{theme}.png','15',theme)
            run(ROOT/'reference/target/release/math-reference',fonts,inp,render_dir/f'{row["id"]:03}-{theme}-ratex.png',theme)
    contact = ROOT/'target/math-contact'
    run('swiftc','-warnings-as-errors',ROOT/'render/contact.swift','-o',contact)
    run(contact,render_dir)
    # Extra cases exercise unfilled paths and dashed horizontal rules.
    for row in [x for x in rows if x['id'] in [99,119] and x['style']=='display']:
        inp = render_dir/f'{row["id"]:03}.json'; write(inp,row)
        for theme in ['light','dark']:
            run(swift,'formula',fonts,inp,render_dir/f'{row["id"]:03}-{theme}.png','15',theme)
            run(ROOT/'reference/target/release/math-reference',fonts,inp,render_dir/f'{row["id"]:03}-{theme}-ratex.png',theme)
    inline = [json.loads(run(ROOT/'target/release/math-spike','once',mode,latex)) | {'style':mode} for mode in ['text','display'] for latex in ['x^2',r'\frac{a}{b}',r'\sum_{i=1}^n']]
    write(out/'inline.json',inline)
    run(swift,'inline',fonts,out/'inline.json',render_dir/'inline-baselines.png')
    chinese = r'\text{概率}=\frac{\text{成功次数}}{\text{总次数}}'
    run(swift,'metrics',fonts,'概率成功次数总速度中文测试🙂',out/'coretext-metrics.json')
    default = json.loads(run(ROOT/'target/release/math-spike','once','display',chinese)); write(out/'chinese-default.json',default)
    host = json.loads(run(ROOT/'target/release/math-spike','fallback',out/'coretext-metrics.json',chinese)); write(out/'chinese-host.json',host)
    for mode in ['default','host']:
        run(swift,'formula',fonts,out/f'chinese-{mode}.json',render_dir/f'chinese-{mode}.png','15','light')

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('mode',choices=['collect','bench','render'])
    p.add_argument('--samples',type=Path,required=True)
    p.add_argument('--markdown',type=Path)
    p.add_argument('--output',type=Path,default=ROOT/'target/reports')
    args=p.parse_args()
    os.environ['PATH']=str(Path.home()/'.rustup/toolchains/stable-aarch64-apple-darwin/bin')+':'+os.environ['PATH']
    args.output.mkdir(parents=True,exist_ok=True)
    globals()[args.mode](args,args.output.resolve())

if __name__=='__main__': main()
