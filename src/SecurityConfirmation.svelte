<script lang="ts">
 import {t} from './locale.svelte';
 import {onMount} from 'svelte';

 let {title,detail,action,oncancel,onconfirm}:{
  title:string;detail:string;action:string;oncancel:()=>void;onconfirm:()=>void
 }=$props();
 const headingId=$props.id();
 let dialog:HTMLDialogElement;
 let cancelButton:HTMLButtonElement;

 onMount(()=>{
  const opener=document.activeElement;
  dialog.showModal();
  cancelButton.focus();
  return ()=>{
   if(dialog.open)dialog.close();
   if(opener instanceof HTMLElement&&opener.isConnected)opener.focus({preventScroll:true});
  };
 });
</script>

<dialog class="security-confirmation dialog-surface" bind:this={dialog} aria-labelledby={headingId}
 aria-describedby={headingId+'-description'} oncancel={event=>{event.preventDefault();oncancel()}}>
 <h2 id={headingId}>{title}</h2>
 <div id={headingId+'-description'}>
  <p><bdi>{detail}</bdi></p>
  <p>{t("Opening email content always carries a security risk. You are responsible for verifying that you trust the sender and know the content is safe before continuing.")}</p>
 </div>
 <div class="security-confirmation-actions">
  <button type="button" class="secondary-button" bind:this={cancelButton} onclick={oncancel}>{t("Cancel")}</button>
  <button type="button" class="reply" onclick={onconfirm}>{action}</button>
 </div>
</dialog>

<style>
 .security-confirmation{width:min(440px,calc(100% - 32px));max-height:calc(100% - 32px);padding:24px;overflow:auto}
 .security-confirmation::backdrop{background:#25301d66;backdrop-filter:blur(3px)}
 .security-confirmation h2{margin:0 0 12px;font-size:20px;font-weight:600}
 .security-confirmation p{margin:0 0 12px;font-size:13px;line-height:1.55;overflow-wrap:anywhere}
 .security-confirmation-actions{display:flex;flex-wrap:wrap;justify-content:flex-end;gap:8px;margin-top:18px}
 .security-confirmation-actions button{min-height:36px;padding:8px 12px;font-size:12px}
</style>
