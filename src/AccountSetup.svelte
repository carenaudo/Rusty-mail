<script lang="ts">
 import {t} from './locale.svelte';
 import {onMount} from 'svelte';
 import {invoke} from '@tauri-apps/api/core';
 import {desktop} from './mail';
 let {onchange=()=>{}}:{onchange?:()=>void}=$props();
 type Account={id:string;provider:string;email:string};
 let accounts:Account[]=$state([]),pending=$state(false),status=$state(''),error=$state(''),custom=$state(false),preset=$state('custom');
 let email=$state(''),imapHost=$state(''),imapPort=$state(993),username=$state(''),smtpHost=$state(''),smtpPort=$state(587),smtpUsername=$state(''),starttls=$state(true),password=$state(''),smtpPassword=$state(''),ca=$state('');
 async function reload(){if(desktop)accounts=await invoke<Account[]>('list_accounts')}
 onMount(()=>{reload().catch(e=>error=String(e))});
 async function connect(provider:'google'|'microsoft'){
   error='';status='';if(!desktop){error=t('Open the desktop app to sign in securely through your system browser.');return}
   pending=true;status=t('Finish signing in in your browser. Rusty Mail never receives your provider password.');
   try{const a=await invoke<Account>('sign_in',{provider});await reload();onchange();status=t('Signed in as {email}. Use Synchronize to download mail.',{email:a.email})}catch(e){status='';error=String(e)}finally{pending=false}
 }
 function choose(){if(preset==='icloud'){imapHost='imap.mail.me.com';smtpHost='smtp.mail.me.com'}else if(preset==='fastmail'){imapHost='imap.fastmail.com';smtpHost='smtp.fastmail.com'}else{imapHost='';smtpHost=''}imapPort=993;smtpPort=587;starttls=true}
 async function connectServer(){if(!desktop){error=t('Custom servers require the desktop app');return}pending=true;error='';status=t('Validating the encrypted server connection…');try{await invoke('connect_imap',{config:{email,imap_host:imapHost,imap_port:imapPort,username:username||email,smtp_host:smtpHost,smtp_port:smtpPort,smtp_username:smtpUsername||username||email,smtp_starttls:starttls,ca_pem:ca||null},password,smtpPassword:smtpPassword||password});password='';smtpPassword='';custom=false;await reload();onchange();status=t('Server connected. Use Synchronize to download mail.')}catch(e){error=String(e);status=''}finally{pending=false}}
 async function sync(id:string){pending=true;try{const n=await invoke<number>('sync_account',{id});status=t('Synchronized {count} messages. Newest mail downloads first.',{count:n});onchange()}catch(e){error=String(e)}finally{pending=false}}
</script>
<div class="account-setup">
 <h3>{t("Email accounts")}</h3><p>{t("Google and Microsoft open their official login website in your default browser, preserving passkeys and multi-factor authentication.")}</p>
 <div class="login-options"><button disabled={pending} onclick={()=>connect('google')}>{t("Continue with Google ↗")}</button><button disabled={pending} onclick={()=>connect('microsoft')}>{t("Continue with Microsoft ↗")}</button></div>
 <button class="custom-toggle" disabled={pending} onclick={()=>custom=!custom}>{t("Add iCloud, Fastmail or custom IMAP/SMTP")}</button>
 {#if pending}<button onclick={()=>invoke('cancel_sign_in')}>{t("Cancel browser sign-in")}</button>{/if}
 {#if status}<p role="status">{status}</p>{/if}{#if error}<p class="login-error" role="alert">{error}</p>{/if}
 {#each accounts as a}<div class="connected-account"><strong>{a.email}</strong><small>{a.provider} {t("· encrypted local cache")}</small><button disabled={pending} onclick={()=>sync(a.id)}>{t("Synchronize")}</button></div>{/each}
 {#if custom}<div class="server-form"><label>{t("Provider")}<select class="option-select" bind:value={preset} onchange={choose}><option value="custom">{t("Custom server")}</option><option value="icloud">{t("iCloud (app-specific password)")}</option><option value="fastmail">{t("Fastmail (app password)")}</option></select></label><p>{t("Use the mail credential or app password provided by your server. Never enter a Google, Microsoft or Apple primary-account password. Credentials are stored in Windows Credential Manager.")}</p><label>{t("Email address")}<input type="email" autocomplete="off" bind:value={email}/></label><label>{t("Incoming IMAP hostname")}<input bind:value={imapHost} placeholder="imap.example.com"/></label><label>{t("IMAP port (implicit TLS)")}<input type="number" min="1" max="65535" bind:value={imapPort}/></label><label>{t("IMAP username")}<input bind:value={username} placeholder={t("Defaults to email address")}/></label><label>{t("Mail/app password")}<input type="password" autocomplete="new-password" bind:value={password}/></label><label>{t("Outgoing SMTP hostname")}<input bind:value={smtpHost} placeholder="smtp.example.com"/></label><label>{t("SMTP port")}<input type="number" min="1" max="65535" bind:value={smtpPort}/></label><label>{t("SMTP username")}<input bind:value={smtpUsername} placeholder={t("Defaults to IMAP username")}/></label><label>{t("SMTP password (blank reuses mail credential)")}<input type="password" autocomplete="new-password" bind:value={smtpPassword}/></label><label>{t("SMTP encryption")}<select class="option-select" bind:value={starttls}><option value={true}>{t("Required STARTTLS")}</option><option value={false}>{t("Implicit TLS")}</option></select></label><details><summary>{t("Private server certificate authority")}</summary><label>{t("Custom CA certificate (PEM)")}<textarea rows="5" bind:value={ca}></textarea></label><p>{t("Certificate and hostname validation always remain enabled.")}</p></details><button class="reply" disabled={pending} onclick={connectServer}>{t("Validate and connect")}</button></div>{/if}
 <details><summary>{t("Other provider requirements")}</summary><p>{t("Yahoo/AOL browser OAuth remains pending the provider’s app registration. No primary-password fallback is offered.")}</p><p>{t("iCloud: generate an app-specific password at account.apple.com when direct Apple authorization is unavailable for this client.")}</p><p>{t("IMAP requires implicit TLS (normally 993). SMTP requires STARTTLS or implicit TLS. Plaintext authentication without TLS is unsupported.")}</p></details>
</div>
<style>
 .account-setup{border-top:1px solid var(--ui-border);margin-top:20px;padding-top:10px}
 h3{font-size:14px;font-weight:550}
 .login-options{display:flex;gap:10px;flex-wrap:wrap}
 .login-options button,.custom-toggle{
  padding:10px;min-height:36px;border:1px solid var(--ui-border);border-radius:7px;
  font-size:12px;flex:1}
 .custom-toggle{margin-top:10px}
 .login-error{color:#b05742!important}
 .connected-account{padding:12px 0;font-size:12px}
 .connected-account small{display:block;margin:4px 0;color:var(--ui-secondary)}
 .connected-account button{border:1px solid var(--ui-border);padding:7px}
 details{margin:20px 0;font-size:12px}
 summary{cursor:pointer}
</style>
