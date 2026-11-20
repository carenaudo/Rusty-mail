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

<dialog class="security-confirmation" bind:this={dialog} aria-labelledby={headingId}
 aria-describedby={headingId+'-description'} oncancel={event=>{event.preventDefault();oncancel()}}>
 <h2 id={headingId}>{title}</h2>
 <div id={headingId+'-description'}>
  <p><bdi>{detail}</bdi></p>
  <p>{t("Opening email content always carries a security risk. You are responsible for verifying that you trust the sender and know the content is safe before continuing.")}</p>
 </div>
 <div class="security-confirmation-actions">
  <button type="button" bind:this={cancelButton} onclick={oncancel}>{t("Cancel")}</button>
  <button type="button" onclick={onconfirm}>{action}</button>
 </div>
</dialog>

<style>
 .security-confirmation{width:min(440px,calc(100% - 32px));max-height:calc(100% - 32px);padding:24px;border:1px solid #dfe5d7;border-radius:12px;background:#f8faf4;color:#232c20;box-shadow:0 16px 48px #10180d50;overflow:auto}
 .security-confirmation::backdrop{background:#131c1299}
 .security-confirmation h2{margin:0 0 12px;font-size:19px}
 .security-confirmation p{margin:0 0 12px;font-size:14px;line-height:1.5;overflow-wrap:anywhere}
 .security-confirmation-actions{display:flex;flex-wrap:wrap;justify-content:flex-end;gap:8px;margin-top:18px}
 .security-confirmation-actions button{padding:8px 12px;border:1px solid currentColor;border-radius:6px}
 .security-confirmation-actions button:last-child{background:#465b36;color:#fff;border-color:transparent}
 :global(.dark) .security-confirmation{background:#242d21;color:#eef3e6;border-color:#59684e}
 :global(.dark) .security-confirmation-actions button:last-child{background:#d7e4c7;color:#222b1c}
</style>
