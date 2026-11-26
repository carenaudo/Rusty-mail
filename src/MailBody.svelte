<script lang="ts">
 import {t} from './locale.svelte';
 import {onDestroy,untrack} from 'svelte';
 import {invoke} from '@tauri-apps/api/core';
 import {prepareMail,mailDocument} from './mail-html';
 import SecurityConfirmation from './SecurityConfirmation.svelte';

 type ImageKind='embedded'|'remote';
 type ImageResult={source:string;data_url:string|null};
 type Confirmation={kind:ImageKind;sources:string[]}|{kind:'link';url:string};
 const maxImageBytes=4*1024*1024;
 let {id,html,text}:{id:string;html:string;text:string}=$props();
 let plain=$state(false),larger=$state(false),linksEnabled=$state(false);
 let images=$state.raw<Record<string,string>>({});
 let attempted=$state.raw<ReadonlySet<ImageKind>>(new Set());
 let loading=$state<ImageKind|null>(null);
 let linkError=$state('');
 let confirmation=$state.raw<Confirmation|null>(null);
 let requestVersion=0;

 const prepared=$derived(prepareMail(html));
 const embedded=$derived(prepared.images.filter(source=>!source.startsWith('https:')));
 const remote=$derived(prepared.images.filter(source=>source.startsWith('https:')));
 const missingEmbedded=$derived(embedded.filter(source=>!images[source]));
 const missingRemote=$derived(remote.filter(source=>!images[source]));
 const loaded=$derived(Object.keys(images).length);
 const failed=$derived((attempted.has('embedded')?missingEmbedded.length:0)
  +(attempted.has('remote')?missingRemote.length:0));
 const documentHtml=$derived(mailDocument(prepared,images,t('Image blocked'),linksEnabled));

 $effect(()=>{
  id;html;
  plain=false;
  larger=false;
  linksEnabled=false;
  linkError='';
  confirmation=null;
  untrack(hideImages);
 });
 onDestroy(()=>{requestVersion++});

 function byteLength(dataUrl:string){
  const payloadLength=dataUrl.length-dataUrl.indexOf(',')-1;
  const padding=dataUrl.endsWith('==')?2:dataUrl.endsWith('=')?1:0;
  return Math.floor(payloadLength*3/4)-padding;
 }
 async function loadImages(kind:ImageKind,sources:string[]){
  if(loading||!sources.length)return;
  const version=++requestVersion;
  loading=kind;
  try{
   const results=await invoke<ImageResult[]>('load_message_images',{id,sources});
   if(version!==requestVersion)return;
   const next={...images};
   const requested=new Set(sources);
   const existingBytes=Object.values(next).reduce((total,dataUrl)=>total+byteLength(dataUrl),0);
   let bytes=existingBytes;
   for(const {source,data_url} of results){
    if(!data_url||!requested.has(source)||next[source])continue;
    const size=byteLength(data_url);
    if(size<=0||bytes+size>maxImageBytes)continue;
    next[source]=data_url;
    bytes+=size;
   }
   if(bytes>existingBytes)images=next;
  }catch{
   // A failed request leaves existing images visible and the missing ones retryable.
  }finally{
   if(version===requestVersion){
    attempted=new Set([...attempted,kind]);
    loading=null;
   }
  }
 }
 function hideImages(){
  requestVersion++;
  if(loaded)images={};
  if(attempted.size)attempted=new Set();
  loading=null;
 }
 function changeFormat(){
  plain=!plain;
  linksEnabled=false;
  confirmation=null;
  hideImages();
 }
 function handleFrameLoad(event:Event){
  if(!linksEnabled)return;
  const frame=event.currentTarget as HTMLIFrameElement;
  const links=prepared.links;
  frame.contentDocument?.addEventListener('click',event=>{
   const anchor=(event.target as Element|null)?.closest?.('a[data-mail-link][href]');
   if(!anchor)return;
   event.preventDefault();
   if(!frame.isConnected||!linksEnabled)return;
   const link=links[Number(anchor.getAttribute('data-mail-link'))];
   if(link)confirmation={kind:'link',url:link.url};
  });
 }
 function confirmContent(){
  const pending=confirmation;
  confirmation=null;
  if(!pending)return;
  if(pending.kind==='link')void openLink(pending.url);
  else void loadImages(pending.kind,pending.sources);
 }
 async function openLink(url:string){
  const messageId=id;
  try{await invoke('open_message_link',{url})}
  catch(e){if(id===messageId)linkError=String(e)}
 }
</script>

<div class="mail-body">
 <div class="mail-reading-options" role="group" aria-label={t("Message view")}>
  <span>{plain?t('Plain text'):t('Formatted message')}</span>
  <button onclick={changeFormat}>{plain?t('Show formatting'):t('View plain text')}</button>
  {#if !plain}<button aria-pressed={larger} onclick={()=>larger=!larger}>{t("Larger view")}</button>{/if}
  {#if !plain&&prepared.links.length}
   <button aria-pressed={linksEnabled} onclick={()=>linksEnabled=!linksEnabled}>{t('Enable links in message')}</button>
  {/if}
 </div>
 {#if plain}
  <div class="body">{text}</div>
 {:else}
  {#if prepared.imageCount}
   <div class="mail-image-notice">
    {#if missingRemote.length}<p>{t("Loading remote images may tell the sender you opened this message.")}</p>{/if}
    <div class="mail-image-actions" role="group" aria-label={t("Message images")}>
     {#if missingEmbedded.length}
      <button disabled={loading!==null} onclick={()=>confirmation={kind:'embedded',sources:missingEmbedded}}>
       {attempted.has('embedded')?t('Retry embedded images'):t('Show embedded images')}
      </button>
     {/if}
     {#if missingRemote.length}
      <button disabled={loading!==null} onclick={()=>confirmation={kind:'remote',sources:missingRemote}}>
       {attempted.has('remote')?t('Retry remote images'):t('Load remote images')}
      </button>
     {/if}
     {#if loaded||loading||attempted.size}<button onclick={hideImages}>{t("Hide images")}</button>{/if}
    </div>
    <div class="mail-image-status" role="status">
     {#if loading}{t(loading==='embedded'?'Loading embedded images…':'Loading remote images…')}
     {:else if failed}{t(failed===1?'{count} image could not be loaded.':'{count} images could not be loaded.',{count:failed})}
     {:else if loaded}{t("Images shown.")}
     {/if}
    </div>
    {#if prepared.skippedImages}<small>{t(prepared.skippedImages===1?'{count} image is unavailable because of the format or image limit.':'{count} images are unavailable because of the format or image limit.',{count:prepared.skippedImages})}</small>{/if}
   </div>
  {/if}
  {#key linksEnabled}
   <iframe class="mail-html-frame" class:larger title={t("Formatted message")} sandbox={linksEnabled?'allow-same-origin':''} referrerpolicy="no-referrer" srcdoc={documentHtml} onload={handleFrameLoad}></iframe>
  {/key}
  {#if prepared.links.length}
   <details class="mail-links">
    <summary>{t('Links in this message ({count})',{count:prepared.links.length})}</summary>
    {#each prepared.links as link,i}
     <button onclick={()=>confirmation={kind:'link',url:link.url}}><span>{i+1}. {link.label}</span><small>{link.url}</small></button>
    {/each}
   </details>
  {/if}
  {#if linkError}<p role="status">{linkError}</p>{/if}
 {/if}
</div>
{#if confirmation}
 <SecurityConfirmation
  title={confirmation.kind==='link'?t('Follow this link?'):confirmation.kind==='remote'?t('Load remote images?'):t('Show embedded images?')}
  detail={confirmation.kind==='link'?confirmation.url:confirmation.kind==='remote'
   ?t('Loading these images contacts external servers and may tell the sender you opened this message.')
   :t('These images are included with the message and can contain unsafe content.')}
  action={confirmation.kind==='link'?t('Follow link'):confirmation.kind==='remote'?t('Load remote images'):t('Show embedded images')}
  oncancel={()=>confirmation=null}
  onconfirm={confirmContent}
 />
{/if}
