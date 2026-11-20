import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { existsSync, readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve, relative } from 'node:path';
import { createHash } from 'node:crypto';

export default defineConfig({plugins:[svelte(),{
 name:'frontend-license-inventory',apply:'build',
 generateBundle(_options,bundle){
  const packages=new Map<string,{name:string;version:string;modules:string[]}>();
  for(const chunk of Object.values(bundle)){
   if(chunk.type!=='chunk')continue;
   for(const [id,module] of Object.entries(chunk.modules)){
    if(!id.includes('/node_modules/')||id.startsWith('\0')||module.renderedLength===0)continue;
    const file=id.split('?')[0];let directory=dirname(file);
    while(directory!==dirname(directory)){
     const manifest=resolve(directory,'package.json');
     if(existsSync(manifest)){
      const pkg=JSON.parse(readFileSync(manifest,'utf8'));
      if(pkg.name&&pkg.version){const key=`${pkg.name}@${pkg.version}`;const entry=packages.get(key)??{name:pkg.name,version:pkg.version,modules:[]};entry.modules.push(relative(process.cwd(),file).replaceAll('\\','/'));packages.set(key,entry);break;}
     }
     directory=dirname(directory);
    }
   }
  }
  if(!packages.size)throw Error('No frontend dependency inventory was produced');
  mkdirSync('reports',{recursive:true});
  writeFileSync('reports/frontend-packages.json',JSON.stringify({scope:'npm modules with rendered code in the production bundle',lockfileSha256:createHash('sha256').update(readFileSync('package-lock.json')).digest('hex'),packages:[...packages.values()].sort((a,b)=>a.name.localeCompare(b.name))},null,2));
 }
}],server:{port:1420,strictPort:true,watch:{ignored:['**/src-tauri/**']}},clearScreen:false});
