// Representative unformatted snippets for the seven languages rt formats
// (src/editor/code-tools.ts). `lang` is the fence tag a user would type.
// Running this file writes corpus/<name>.<lang> for the non-JS runners.
export type Case = { name: string; lang: string; input: string };

export const cases: Case[] = [
  { name: "rt-media-code-test", lang: "js", input: "const x={a:1}" },
  {
    name: "js-object-long",
    lang: "javascript",
    input: `const config = {name:'notes', version:2, plugins:[require('a'),require("b")], enabled:true, nested:{deep:{value:[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20]}}}`,
  },
  {
    name: "js-async-chain",
    lang: "js",
    input: `async function load(id){const res=await fetch(\`/api/pages/\${id}\`,{headers:{'Accept':'application/json'}});if(!res.ok)throw new Error('failed '+res.status)
return res.json().then(data=>data.items.filter(i=>i.visible).map(i=>({...i,title:i.title.trim()})))}`,
  },
  {
    name: "js-class-comments",
    lang: "js",
    input: `// Keeps a bounded cache
class Lru { constructor(limit=10){ this.limit=limit; this.map=new Map() } // ctor
  get(k){ if(!this.map.has(k)) return undefined; const v=this.map.get(k); this.map.delete(k); this.map.set(k,v); return v }
  /* evict oldest */ set(k,v){ this.map.delete(k); this.map.set(k,v); if(this.map.size>this.limit){ this.map.delete(this.map.keys().next().value) } } }
export default Lru`,
  },
  {
    name: "jsx-component",
    lang: "jsx",
    input: `export function List({items,onPick}){return <ul className="list">{items.map(item=><li key={item.id} onClick={()=>onPick(item)}>{item.label} <span className="count">{item.count}</span></li>)}</ul>}`,
  },
  {
    name: "js-ternary-chain",
    lang: "js",
    input: `const label = n===0?'none':n===1?'one':n<10?'a few':'many'; let a=1,b=2;;[a,b]=[b,a]
if (a) b++
else { b-- }`,
  },
  {
    name: "ts-interface-generic",
    lang: "typescript",
    input: `interface Page<T extends object = {}> {id:string; title?:string; meta:T; children: Array<Page<T>>}
type Result<T,E=Error> = {ok:true,value:T}|{ok:false,error:E}
export function walk<T extends object>(page:Page<T>, visit:(p:Page<T>,depth:number)=>void, depth=0):void{visit(page,depth);for(const c of page.children)walk(c,visit,depth+1)}`,
  },
  {
    name: "ts-enum-class",
    lang: "ts",
    input: `enum Status {Draft='draft', Saved='saved', Failed='failed'}
export abstract class Store<K extends string, V> implements Iterable<[K,V]>{ private readonly items=new Map<K,V>(); constructor(protected readonly name:string){}
  abstract validate(value:V):value is V
  *[Symbol.iterator](){ yield* this.items } }
const s = <Store<string, number>>(null as unknown); const n = s!.name as const satisfies string`,
  },
  {
    name: "tsx-component",
    lang: "tsx",
    input: `import {useState} from 'react'
export const Counter: React.FC<{start?:number}> = ({start=0}) => { const [n,setN]=useState<number>(start); return <button type="button" onClick={()=>setN(v=>v+1)} disabled={n>=10}>Clicked {n} times</button> }`,
  },
  {
    name: "json-object",
    lang: "json",
    input: `{"english":"A clear next step","urdu":"اگلا قدم","arabic":"الخطوة التالية","list":[1,2,3],"nested":{"a":null,"b":true,"c":[{"d":1.50},{"e":"x"}]}}`,
  },
  {
    name: "json-long-array",
    lang: "json",
    input: `{"name":"quire","scripts":{"test":"vitest run","build":"vite build"},"keywords":["notes","markdown","outliner","sync","native","desktop","electron","gpui"],"empty":{},"emptyArr":[]}`,
  },
  {
    name: "json-comments-trailing",
    lang: "json",
    input: `// settings
{
    "tabWidth" : 4, /* inline */
  "items": [ 1,
   2, ],
}`,
  },
  {
    name: "css-rules",
    lang: "css",
    input: `.pdf-embed{display:block;width:100%;margin:12px 0;border:1px solid var(--line);background:var(--paper)}
.pdf-embed iframe,.image-embed img{display:block;width:100%;height:650px;max-height:80vh;border:0}
@media (max-width:600px){.toolbar>button:hover:not(.active){color:#FFF;transition:color .2s ease-in-out,background-color .2s ease-in-out}}`,
  },
  {
    name: "css-complex",
    lang: "css",
    input: `:root{--accent:#356B5B;--font:"Inter",system-ui,-apple-system,sans-serif}
a[href^='http']::after{content:"\\2197";margin-left:.25em}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));grid-template-areas:"head head" "side main"}
@supports (backdrop-filter:blur(4px)){.panel{backdrop-filter:blur(4px)}}
/* comment */ @keyframes spin{from{transform:rotate(0deg)}to{transform:rotate(360DEG)}}`,
  },
  {
    name: "html-page",
    lang: "html",
    input: `<!DOCTYPE html><html><head><meta charset="utf-8"><title>Notes</title><link rel="stylesheet" href="style.css"></head><body><div class="app"><h1>Today</h1><p>Some <b>bold</b> and <i>italic</i> text with a <a href="https://example.com">link</a>.</p><ul><li>One</li><li>Two</li></ul></div><script>console.log("hi")</script></body></html>`,
  },
  {
    name: "html-fragment",
    lang: "html",
    input: `<form action="/save" method="post"><label for="t">Title</label><input id="t" name="title" type="text" required placeholder="Untitled"><select name="kind"><option value="a" selected>A</option><option value="b">B</option></select><button type="submit" class="primary" disabled>Save</button></form>`,
  },
  {
    name: "xml-alias",
    lang: "xml",
    input: `<note><to>Tove</to><from>Jani</from><heading>Reminder</heading><body>Don't forget me this weekend!</body></note>`,
  },
  {
    name: "yaml-config",
    lang: "yaml",
    input: `name:   quire
version: 0.9.57
targets: [macos,   linux, windows]
build:
    release: true
    flags:
       - --locked
       -   --release
env: {RUST_LOG: info,   CARGO_HOME: /tmp/cargo}
# trailing comment
description: >
   A folded
   block scalar`,
  },
  {
    name: "yml-workflow",
    lang: "yml",
    input: `on:
  push:
      branches: [ main ]
jobs:
  test:
     runs-on: \${{ matrix.os }}
     strategy:
        matrix:
            os: [ubuntu-latest,macos-latest]
     steps:
        - uses: actions/checkout@v4
        - run: cargo test --locked
          env:
              CI: "true"
---
second: doc`,
  },
  {
    name: "markdown-mixed",
    lang: "markdown",
    input: `Heading
=======
* item one
* item two
    * nested item
1) first
2) second

Some *emphasis* and __strong__ text that is fairly long and keeps going past the usual eighty character print width limit.

|a|b|
|-|:-:|
|long cell value|x|

\`\`\`js
const x={a:1}
\`\`\``,
  },
  {
    name: "md-notes",
    lang: "md",
    input: `# Meeting   notes
- [ ] Ask about **budget**
- [x] Send   agenda
+ mixed marker

> quoted
continued

Line with trailing spaces
next line\\
[link]( https://example.com )   and ![img](a.png "title")`,
  },
];

if (import.meta.main) {
  const dir = new URL("./corpus/", import.meta.url).pathname;
  await Bun.$`mkdir -p ${dir}`;
  for (const c of cases) await Bun.write(`${dir}${c.name}.${c.lang}`, c.input);
  console.log(`wrote ${cases.length} cases to ${dir}`);
}
