// SPDX-License-Identifier: MIT OR Apache-2.0
import fs from 'node:fs/promises';
import path from 'node:path';
import {createRequire} from 'node:module';
import {parseArgs} from 'node:util';

const {values} = parseArgs({options:{
  results:{type:'string'}, katex:{type:'string'},
  output:{type:'string',default:'target/reports/katex-compatibility.json'},
}});
if (!values.results || !values.katex) throw Error('--results results.jsonl --katex /path/to/node_modules/katex required');
const require = createRequire(import.meta.url);
const directory = path.resolve(values.katex);
const katex = require(directory);
require(path.join(directory,'dist/contrib/mhchem.js'));
const rows = (await fs.readFile(values.results,'utf8')).trimEnd().split('\n').map(JSON.parse);
const options = {throwOnError:true,strict:'ignore',trust:false};
const results = rows.map(row=>{
  const result = {id:row.id,input:row.input,style:row.style,ok:false,ratex_ok:row.validate_ok};
  try {
    const html = katex.renderToString(row.input,{...options,displayMode:row.style==='display',macros:{}});
    result.ok = true; result.output_length = html.length;
  } catch(error) { result.error = error.message; }
  return result;
});
await fs.mkdir(path.dirname(path.resolve(values.output)),{recursive:true});
await fs.writeFile(values.output,JSON.stringify({version:katex.version,extensions:['mhchem'],options,results},null,2)+'\n');
console.log(JSON.stringify({version:katex.version,total:results.length,differences:results.filter(x=>x.ok!==x.ratex_ok).length}));
