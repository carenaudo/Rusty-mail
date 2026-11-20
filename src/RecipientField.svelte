<script lang="ts">
 import {t} from './locale.svelte';
 export type RecentRecipient = {address:string;formatted:string};
 let {label, value=$bindable(), recent, hint=''}:{
  label:string;value:string;recent:RecentRecipient[];hint?:string
 }=$props();
 const fieldId=$props.id();
 const listId=fieldId+'-recipients';
 let input:HTMLInputElement;
 let start=$state(0),end=$state(0),active=$state(-1),visible=$state(false);
 let term=$derived(value.slice(start,end).trim().toLowerCase());
 let choices=$derived(recent.filter(recipient=>
  (recipient.address.toLowerCase().includes(term)||recipient.formatted.toLowerCase().includes(term))
  && recipient.formatted.toLowerCase()!==term
 ).slice(0,6));
 let expanded=$derived(visible&&choices.length>0);

 function updateRange(){
  const cursor=input.selectionStart??value.length;
  let quoted=false, escaped=false, angle=false, commentDepth=0;
  start=0;end=value.length;active=-1;visible=true;
  for(let index=0;index<value.length;index++){
   const char=value[index];
   if(escaped){escaped=false;continue}
   if(char==='\\'&&(quoted||commentDepth)){escaped=true;continue}
   if(char==='"'&&!commentDepth){quoted=!quoted;continue}
   if(quoted)continue;
   if(char==='('){commentDepth++;continue}
   if(char===')'&&commentDepth){commentDepth--;continue}
   if(commentDepth)continue;
   if(char==='<')angle=true;
   if(char==='>')angle=false;
   if(char===','&&!angle){
    if(index>=cursor){end=index;break}
    start=index+1;
   }
  }
 }
 function choose(recipient:RecentRecipient){
  const previous=value.slice(0,start);
  const before=previous+(previous?' ':'')+recipient.formatted;
  value=before+value.slice(end);
  visible=false;active=-1;
  input.focus();
  queueMicrotask(()=>input.setSelectionRange(before.length,before.length));
 }
 function keyboard(event:KeyboardEvent){
  if(event.key==='Escape'&&expanded){
   event.stopPropagation();
   visible=false;
   return;
  }
  if(!expanded)return;
  if(event.key==='ArrowDown'||event.key==='ArrowUp'){
   event.preventDefault();
   active=event.key==='ArrowDown'
    ?(active+1)%choices.length
    :(active<=0?choices.length-1:active-1);
  }else if(event.key==='Enter'&&active>=0&&choices[active]){
   event.preventDefault();
   choose(choices[active]);
  }
 }
</script>

<div class="recipient-field">
 <label>{label}
  <input type="text" bind:this={input} bind:value oninput={()=>queueMicrotask(updateRange)}
   onfocus={updateRange} onclick={updateRange} onblur={()=>visible=false}
   onkeyup={event=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(event.key))updateRange()}}
   onkeydown={keyboard} role="combobox" aria-autocomplete="list" aria-expanded={expanded}
   aria-controls={listId} aria-activedescendant={expanded&&active>=0?listId+'-'+active:undefined}
   autocomplete="off" spellcheck="false"/>
  {#if hint}<small>{hint}</small>{/if}
 </label>
 <div id={listId} class="recipients" role="listbox" aria-label={t("Recent recipients")} hidden={!expanded}>
  {#each choices as recipient,index (recipient.address)}
   <button id={listId+'-'+index} type="button" tabindex="-1" role="option"
    aria-selected={active===index} onpointerdown={event=>event.preventDefault()}
    onclick={()=>choose(recipient)}>{recipient.formatted}</button>
  {/each}
 </div>
</div>

<style>
 .recipient-field{position:relative}
 .recipients{position:absolute;z-index:3;top:100%;left:0;right:0;max-height:170px;overflow:auto;
  padding:4px;border:1px solid #87927777;border-radius:6px;background:#fffefa;color:#455639;
  box-shadow:0 4px 12px #0002}
 .recipients button{display:block;width:100%;padding:7px;text-align:left;font-size:12px;overflow-wrap:anywhere}
 .recipients button[aria-selected=true]{background:#edf2e6}
 :global(.dark) .recipients{background:#22291f;color:#c5cfba}
 :global(.dark) .recipients button[aria-selected=true]{background:#303e27}
</style>
