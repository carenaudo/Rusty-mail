import DOMPurify from 'dompurify';

const tags=['p','div','span','br','hr','strong','b','em','i','u','s','del','small','sub','sup','blockquote','pre','code','ul','ol','li','dl','dt','dd','h1','h2','h3','h4','h5','h6','table','thead','tbody','tfoot','tr','td','th','caption','colgroup','col','a','img'];
const attrs=['alt','title','width','height','colspan','rowspan','dir','lang','scope','style','data-mail-image'];
export const MAX_IMAGES=12;
export type PreparedMail={html:string;images:string[];imageCount:number;skippedImages:number;links:{label:string;url:string}[]};
const config={ALLOWED_TAGS:tags,ALLOWED_ATTR:attrs,ALLOW_DATA_ATTR:false,ALLOW_ARIA_ATTR:false,FORBID_TAGS:['svg','math','style','script','template'],RETURN_DOM_FRAGMENT:true as const};

function safeStyle(style:CSSStyleDeclaration):string {
 const out=document.createElement('span').style;
 for(const property of ['color','background-color','font-size','font-weight','font-style','text-align','text-decoration','vertical-align','white-space','margin-top','margin-bottom','padding','border-width','border-style','border-color','border-collapse','width','max-width']) {
  const value=style.getPropertyValue(property).trim();
  if(!value||value.length>80||!/^[a-z0-9#(),.% \-]+$/i.test(value)||/url|var\(|attr\(|expression|image|\\/i.test(value))continue;
  if(['font-size','margin-top','margin-bottom','padding','border-width','width','max-width'].includes(property)) {
   if(!/^(?:\d{1,3}(?:\.\d{1,2})?(?:px|pt|em|rem|%)?|\s)+$/.test(value))continue;
   const numbers=value.match(/\d+(?:\.\d+)?/g)?.map(Number)??[];
   const limit=/em|rem/.test(value)?(property==='font-size'?2:3):/%/.test(value)?(property==='font-size'?160:100):['width','max-width'].includes(property)?800:property==='font-size'?36:48;
   if(numbers.some(n=>n>limit))continue;
  }
  out.setProperty(property,value);
 }
 return out.cssText;
}
function linkUrl(raw:string):string|null {
 if(raw.length>4096||/[\u0000-\u0020]/.test(raw)||/%0[ad]/i.test(raw))return null;
 try{const u=new URL(raw);return ['https:','http:','mailto:'].includes(u.protocol)&&!u.username&&!u.password?u.href:null}catch{return null}
}
function imageSource(raw:string):string|null {
 const source=raw.trim();
 if(/^https:\/\//i.test(source)){try{const u=new URL(source);return !u.username&&!u.password&&u.port===''&&source.length<=4096?u.href:null}catch{return null}}
 if(/^cid:[^\u0000-\u0020]{1,1024}$/i.test(source))return 'cid:'+source.slice(4);
 if(/^data:image\/(?:png|jpeg|gif|webp);base64,[a-z0-9+/=]+$/i.test(source)&&source.length<=1_000_000){const split=source.indexOf(',');return source.slice(0,split).toLowerCase()+source.slice(split)}
 return null;
}

export function prepareMail(html:string):PreparedMail {
 // Template contents stay inert: parsing must not preload original URLs, even
 // before DOMPurify runs. Strip every URL-bearing attribute before sanitizing.
 const template=document.createElement('template');
 template.innerHTML=html.length<=1_000_000?html:'';
 const images:string[]=[];const links:PreparedMail['links']=[];let imageCount=0,skippedImages=0;
 for(const el of Array.from(template.content.querySelectorAll('*'))) {
  if(!template.content.contains(el))continue;
  const tag=el.localName.toLowerCase();
  if(['script','style','iframe','object','embed','svg','math','template','link','meta','base','audio','video','source'].includes(tag)){el.remove();continue}
  const style=el instanceof HTMLElement?safeStyle(el.style):'';
  const retained=new Map<string,string>();
  for(const name of ['alt','title','width','height','colspan','rowspan','dir','lang','scope']) {
   const value=el.getAttribute(name);
   if(value!==null&&value.length<=512&&(!['width','height','colspan','rowspan'].includes(name)||/^\d{1,3}$/.test(value)))retained.set(name,value);
  }
  let imageIndex=-1;
  if(tag==='img') {
   imageCount++;
   const source=imageSource(el.getAttribute('src')??'');
   if(source){imageIndex=images.indexOf(source);if(imageIndex<0&&images.length<MAX_IMAGES){imageIndex=images.length;images.push(source)}}
   if(imageIndex<0)skippedImages++;
  }
  if(tag==='a') {
   const url=linkUrl(el.getAttribute('href')??'');
   if(url&&links.length<30){links.push({label:el.textContent?.trim().slice(0,100)||url,url});el.append(document.createTextNode(` [${links.length}]`))}
  }
  for(const attr of Array.from(el.attributes))el.removeAttribute(attr.name);
  for(const [name,value]of retained)el.setAttribute(name,value);
  if(style)el.setAttribute('style',style);
  if(tag==='img')el.setAttribute('data-mail-image',String(imageIndex));
 }
 const clean=DOMPurify.sanitize(template.content,config);
 const holder=document.createElement('div');holder.append(clean);
 return {html:holder.innerHTML,images,imageCount,skippedImages,links};
}
export function mailDocument(prepared:PreparedMail,images:Record<string,string>={},blockedLabel='Image blocked'):string {
 const template=document.createElement('template');template.innerHTML=prepared.html;
 for(const node of Array.from(template.content.querySelectorAll('img'))) {
  const index=node.getAttribute('data-mail-image');
  const source=index!==null&&/^\d+$/.test(index)?prepared.images[Number(index)]:undefined;
  const data=source?images[source]:undefined;
  if(data&&/^data:image\/(?:png|jpeg|gif|webp);base64,[a-z0-9+/=]+$/i.test(data)) {
   node.setAttribute('src',data);node.setAttribute('referrerpolicy','no-referrer');
  } else {
   const placeholder=document.createElement('span');
   placeholder.className='mail-image-placeholder';
   placeholder.textContent=`[${blockedLabel}${node.getAttribute('alt')?`: ${node.getAttribute('alt')}`:''}]`;
   node.replaceWith(placeholder);
  }
 }
 const clean=DOMPurify.sanitize(template.content,{...config,ALLOWED_ATTR:[...attrs,'src','referrerpolicy','class']});
 const holder=document.createElement('div');holder.append(clean);
 return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"><meta name="referrer" content="no-referrer"><style>:root{color-scheme:light dark}body{font:16px/1.65 'Segoe UI',sans-serif;overflow-wrap:anywhere;margin:16px}table{border-collapse:collapse;max-width:100%}td,th{padding:5px;overflow-wrap:anywhere}pre{white-space:pre-wrap}blockquote{border-inline-start:3px solid #89977d;margin:16px 0;padding-inline-start:16px}img{max-width:100%;height:auto;max-height:640px;object-fit:contain}a{color:inherit;text-decoration:underline}.mail-image-placeholder{display:block;margin:8px 0;padding:8px 12px;border:1px solid #89977d66;border-radius:4px;font-size:13px}h1,h2,h3{line-height:1.35}hr{border:0;border-top:1px solid #89977d}</style></head><body>${holder.innerHTML}</body></html>`;
}

