import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync,readdirSync,existsSync,mkdirSync,statSync} from 'node:fs';
import {dirname,join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const command=(file,args)=>execFileSync(file,args,{cwd:root,encoding:'utf8',maxBuffer:64*1024*1024});
const metadata=JSON.parse(command('cargo.exe',['metadata','--locked','--format-version','1','--all-features','--manifest-path','src-tauri/Cargo.toml','--filter-platform','x86_64-pc-windows-msvc']));
const packages=new Map(metadata.packages.map(p=>[p.id,p]));
const nodes=new Map(metadata.resolve.nodes.map(n=>[n.id,n]));
const included=new Set();
function walk(id){if(included.has(id))return;included.add(id);for(const dep of nodes.get(id)?.deps??[]){if(dep.dep_kinds.some(k=>k.kind!=='dev'))walk(dep.pkg)}}
walk(metadata.resolve.root);
const records=[];
function licenses(directory){const result=[];if(!existsSync(directory))return result;for(const item of readdirSync(directory,{withFileTypes:true})){if(item.isFile()&&/^(licen[cs]e|copying|notice|unlicense|copyright)([._-]|$)/i.test(item.name)&&statSync(join(directory,item.name)).size<2*1024*1024){result.push({name:item.name,text:readFileSync(join(directory,item.name),'utf8')})}if(item.isDirectory()&&/^licenses?$/i.test(item.name)){for(const file of readdirSync(join(directory,item.name),{withFileTypes:true})){const path=join(directory,item.name,file.name);if(file.isFile()&&statSync(path).size<2*1024*1024)result.push({name:`${item.name}/${file.name}`,text:readFileSync(path,'utf8')})}}}return result;}
for(const id of [...included].sort()){const pkg=packages.get(id);if(!pkg||pkg.id===metadata.resolve.root)continue;const directory=dirname(pkg.manifest_path);const texts=licenses(directory);if(pkg.license_file&&existsSync(join(directory,pkg.license_file))&&!texts.some(t=>t.name===pkg.license_file)){texts.push({name:pkg.license_file,text:readFileSync(join(directory,pkg.license_file),'utf8')})}
 records.push({ecosystem:'Rust',name:pkg.name,version:pkg.version,license:pkg.license??'See license files',source:`https://crates.io/api/v1/crates/${pkg.name}/${pkg.version}/download`,texts});
 if(pkg.name==='libsqlite3-sys'){records.push({ecosystem:'Bundled C',name:'SQLCipher',version:`vendored by ${pkg.name} ${pkg.version}`,license:'BSD-3-Clause',source:pkg.repository,texts:licenses(join(directory,'sqlcipher'))});}
 if(pkg.name==='openssl-src'){records.push({ecosystem:'Bundled C',name:'OpenSSL',version:`vendored by ${pkg.name} ${pkg.version}`,license:'Apache-2.0',source:'https://github.com/openssl/openssl',texts:licenses(join(directory,'openssl'))});}
}
// Include code actually emitted by Vite, rather than compiler-only npm dependencies.
const inventory=JSON.parse(readFileSync(join(root,'reports/frontend-packages.json'),'utf8'));
if(inventory.lockfileSha256!==createHash('sha256').update(readFileSync(join(root,'package-lock.json'))).digest('hex'))throw Error('Run npm run build to refresh the frontend license inventory');
const shipped=new Set(inventory.packages.map(p=>`${p.name}@${p.version}`));
const npmTree=JSON.parse(command('cmd.exe',['/d','/s','/c','npm.cmd ls --omit=dev --all --long --json']));
const seen=new Set();
function npmWalk(node){for(const dep of Object.values(node.dependencies??{})){const directory=dep.path;if(directory&&!seen.has(directory)){seen.add(directory);const pkg=JSON.parse(readFileSync(join(directory,'package.json'),'utf8'));if(shipped.has(`${pkg.name}@${pkg.version}`))records.push({ecosystem:'npm',name:pkg.name,version:pkg.version,license:typeof pkg.license==='string'?pkg.license:'See package license',source:`https://registry.npmjs.org/${pkg.name}/-/${pkg.name.split('/').at(-1)}-${pkg.version}.tgz`,texts:licenses(directory)});}npmWalk(dep)}}
npmWalk(npmTree);
for(const key of shipped){if(!records.some(r=>r.ecosystem==='npm'&&`${r.name}@${r.version}`===key))throw Error(`Missing shipped frontend package: ${key}`);}
const tslib=inventory.packages.find(p=>p.name==='@tauri-apps/api')?.modules.find(p=>p.endsWith('/external/tslib/tslib.es6.js'));
if(tslib){const code=readFileSync(join(root,tslib),'utf8');const end=code.indexOf('*/');if(!code.startsWith('/*')||end<0||!code.slice(0,end).includes('Copyright'))throw Error('Review embedded tslib notice');records.push({ecosystem:'Bundled JavaScript',name:'tslib helpers',version:'vendored by @tauri-apps/api',license:'ISC',source:records.find(r=>r.name==='@tauri-apps/api').source,texts:[{name:'Original source header',text:code.slice(0,end+2)}]});}
// These versions omit license files from their archives. Preserve pinned upstream copies.
const supplemental={
 'alloc-stdlib@0.3.0':'alloc-stdlib-0.3.0-LICENSE',
 'selectors@0.38.0':'MPL-2.0.txt',
 'stop-token@0.7.0':'Apache-2.0.txt',
 'tauri-plugin@2.7.2':'tauri-plugin-2.7.2-LICENSE-MIT',
 'webview2-com@0.39.1':'webview2-edc2caf-LICENSE',
 'webview2-com-sys@0.39.1':'webview2-edc2caf-LICENSE',
 'webview2-com-macros@0.8.1':'webview2-dffa41a-LICENSE',
};
const provenance=JSON.parse(readFileSync(join(root,'third-party-licenses/sources.json'),'utf8').replace(/^\uFEFF/,''));
for(const record of records){const file=supplemental[`${record.name}@${record.version}`];if(!record.texts.length&&file){const source=provenance.find(s=>s.file===file);if(!source)throw Error(`Missing license provenance: ${file}`);record.texts.push({name:`${file} — ${source.url}`,text:readFileSync(join(root,'third-party-licenses',file),'utf8')});}}
let output='# Third-party software notices\n\nGenerated from locked Windows x86_64 dependencies, including optional signed updates, and npm modules emitted by the production Vite build. Original Rusty Mail code is MIT licensed; the following components retain their own licenses. No covered upstream sources have been modified. Exact source archive URLs are supplied for reproduction, including MPL-2.0 components. Rust build dependencies are included conservatively. Compiler-only npm modules are excluded using reports/frontend-packages.json.\n\n';
for(const record of records){output+=`## ${record.name} ${record.version}\n\nEcosystem: ${record.ecosystem}. License: ${record.license}.\n\nSource: ${record.source}\n\n`;if(!record.texts.length){output+='License text is included in the linked source archive; review this component before distribution.\n\n';}for(const text of record.texts){output+=`### ${text.name}\n\n`+text.text.split(/\r?\n/).map(l=>'    '+l).join('\n')+'\n\n';}}
writeFileSync(join(root,'THIRD_PARTY_NOTICES.md'),output);
mkdirSync(join(root,'reports'),{recursive:true});
writeFileSync(join(root,'reports','licenses.json'),JSON.stringify(records.map(({texts,...record})=>({...record,licenseFiles:texts.map(t=>t.name)})),null,2));
console.log(`Generated notices for ${records.length} dependency components.`);
