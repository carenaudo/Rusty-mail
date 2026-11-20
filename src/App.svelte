<script lang="ts">
 import {folderName,t} from './locale.svelte';
 import LanguagePicker from './LanguagePicker.svelte';
 import {onMount} from 'svelte';
 import AccountSetup from './AccountSetup.svelte';
 import LiveApp from './LiveApp.svelte';
 import {Inbox,PenLine,Search,Star,Archive,Trash2,Clock,ChevronRight,ArrowLeft,Settings,MoreHorizontal,Command,Sun,Mail,ArrowUpRight,X,Sparkles,Check,PanelLeftClose} from 'lucide-svelte';
 import {invoke} from '@tauri-apps/api/core';
 import {load,save,desktop,type Message} from './mail';
 let messages:Message[]=$state([]),selected=$state('1'),view=$state('inbox'),category=$state('all'),query=$state(''),focus=$state(false),sidebar=$state(true),settings=$state(false),palette=$state(false),compose=$state(false),error=$state(''),notice=$state(''),ai=$state(''),busy=$state(false),dark=$state(false);
 let to=$state(''),subject=$state(''),body=$state(''),draftKey='rusty-demo-draft';
 let last:Message[]|null=$state(null);
 let listScroll=0;
 let fontSize=$state(15),lineWidth=$state(850),compact=$state(false);
 function restoreScroll(node:HTMLElement){node.scrollTop=listScroll}
 function modalFocus(node:HTMLElement){const previous=document.activeElement as HTMLElement;const fields=()=>Array.from(node.querySelectorAll<HTMLElement>('button,input,textarea,[tabindex]')).filter(e=>!e.hasAttribute('disabled'));queueMicrotask(()=>fields()[0]?.focus());function trap(e:KeyboardEvent){if(e.key!=='Tab')return;const f=fields(),first=f[0],end=f[f.length-1];if(e.shiftKey&&document.activeElement===first){e.preventDefault();end?.focus()}else if(!e.shiftKey&&document.activeElement===end){e.preventDefault();first?.focus()}}node.addEventListener('keydown',trap);return{destroy(){node.removeEventListener('keydown',trap);previous?.focus()}}}
 let visible=$derived(messages.filter(m=>(view==='starred'?m.starred:m.folder===view)&&(category==='all'||m.category===category)&&`${m.sender} ${m.subject} ${m.body} ${m.account}`.toLowerCase().includes(query.toLowerCase())));
 let current=$derived(messages.find(m=>m.id===selected));
 $effect(()=>{if(!visible.some(m=>m.id===selected)){selected=visible[0]?.id||'';ai=''}});
 $effect(()=>{if(!compose)return;const data={to,subject,body};const timer=setTimeout(()=>{if(desktop)invoke('save_draft',data).catch(e=>error=String(e));else localStorage.setItem(draftKey,JSON.stringify(data))},800);return()=>clearTimeout(timer)});
 onMount(()=>{if(desktop)return;load().then(m=>messages=m).catch(e=>error=String(e)); if(desktop){invoke<{to:string;subject:string;body:string}|null>('load_draft').then(d=>{if(d)({to,subject,body}=d)}).catch(e=>error=String(e))}else{const draft=localStorage.getItem(draftKey);if(draft){try{({to,subject,body}=JSON.parse(draft))}catch{}}}});
 async function persist(){try{await save(messages)}catch(e){error=String(e)}}
 function open(m:Message){selected=m.id;ai='';m.unread=false;persist()}
 function mutate(folder:string){if(!current)return;last=structuredClone($state.snapshot(messages));current.folder=folder;notice=folder==='snoozed'?t('Snoozed in this demo. Scheduled wake-up is not implemented.'):t('Moved to {folder}',{folder:folderName(folder)});selected=visible.find(m=>m.id!==selected)?.id||'';persist()}
 function undo(){if(last){messages=last;last=null;notice=t('Undone');persist()}}
 function reply(){if(!current)return;to=current.address;subject='Re: '+current.subject;body='';compose=true}
 async function draft(){try{if(desktop)await invoke('save_draft',{to,subject,body});else localStorage.setItem(draftKey,JSON.stringify({to,subject,body}));notice=t('Draft saved');compose=false}catch(e){error=String(e)}}
 async function summarize(){if(!current)return;const requestedId=current.id;busy=true;ai='';try{if(!desktop)throw new Error(t('AI is available in the desktop app. Configure a local endpoint in the environment.'));const result=await invoke<string>('summarize',{id:requestedId});if(current?.id===requestedId)ai=result}catch(e){error=String(e)}finally{busy=false}}
 function keyboard(e:KeyboardEvent){if(desktop)return;if(e.key==='Escape'){compose=false;settings=false;palette=false;focus=false}if((e.ctrlKey||e.metaKey)&&e.key==='k'){e.preventDefault();palette=!palette;return}if(e.ctrlKey||e.metaKey||e.altKey||compose||settings||palette||(e.target as HTMLElement)?.closest('input,textarea,[contenteditable]'))return;if(e.key==='j'||e.key==='k'){const i=visible.findIndex(m=>m.id===selected);const m=visible[i+(e.key==='j'?1:-1)];if(m)open(m)}if(e.key==='c')compose=true;if(e.key==='e')mutate('archive');if(e.key==='r')reply();if(e.key==='z')undo()}
</script>

<svelte:window onkeydown={keyboard}/>
{#if desktop}<LiveApp/>{:else}
<div class:dark class:compact class="app" class:focused={focus} style={`--reader-size:${fontSize}px;--reader-width:${lineWidth}px`}>
 {#if sidebar&&!focus}<aside>
 <div class="brand"><div class="brand-mark"><Mail size={20}/></div>rusty<span>mail</span></div>
 <button class="compose" onclick={()=>compose=true}><PenLine size={17}/> {t("Compose")} <kbd>C</kbd></button>
 <div class="section-label">{t("YOUR SPACE")}</div>
 <nav>{#each [{id:'inbox',name:t('All inbox'),icon:Inbox},{id:'starred',name:t('Starred'),icon:Star},{id:'snoozed',name:t('Snoozed'),icon:Clock},{id:'archive',name:t('Archive'),icon:Archive},{id:'trash',name:t('Trash'),icon:Trash2}] as item}<button class:active={view===item.id} onclick={()=>{view=item.id;category='all'}}><item.icon size={18}/>{item.name}{#if item.id==='inbox'}<span class="count">{messages.filter(m=>m.unread&&m.folder==='inbox').length}</span>{/if}</button>{/each}</nav>
 <div class="section-label account-heading">{t("ACCOUNTS")} <button aria-label={t("Add email account")} onclick={()=>settings=true}>＋</button></div>
 <button class="account" onclick={()=>query=t('Personal')}><span class="account-dot peach">P</span><div>{t("Personal")}<small>{t("Sample account")}</small></div></button>
 <button class="account" onclick={()=>query=t('Work')}><span class="account-dot purple">W</span><div>{t("Work")}<small>{t("Sample account")}</small></div></button>
 <div class="sidebar-bottom"><div class="local"><span></span> {t("A little calmer, by design.")}</div><button onclick={()=>settings=true}><Settings size={17}/> {t("Settings")} <kbd>⌘ ,</kbd></button></div>
 </aside>{/if}
 <main>
 <header><div class="breadcrumb"><button title={t("Toggle sidebar")} onclick={()=>sidebar=!sidebar}><PanelLeftClose size={18}/></button><span>{t("Your space")}</span><ChevronRight size={14}/><strong>{view==='inbox'?t('All inbox'):folderName(view)}</strong></div><div class="header-actions"><span class="demo-label">{t("DEMO MAILBOX")}</span><button title={t("Command palette")} onclick={()=>palette=true}><Command size={17}/></button><button title={t("Toggle dark theme")} onclick={()=>dark=!dark}><Sun size={18}/></button><div class="avatar">Y</div></div></header>
 <div class="workspace">
 {#if !focus}<section class="message-list"><div class="list-heading"><div><h1>{view==='inbox'?t('All inbox'):folderName(view)}</h1><p>{t("A clear head starts here.")}</p></div><button title={t("Mailbox information")} onclick={()=>notice=t('Sample messages only. Connectors are not yet implemented.')}><MoreHorizontal size={20}/></button></div>
 <label class="search"><Search size={16}/><input aria-label={t("Search downloaded mail")} placeholder={t("Search your mail")} bind:value={query}/><kbd>⌘ K</kbd></label>
 <div class="tabs">{#each ['all','people','newsletters','notifications'] as tab}<button class:chosen={category===tab} onclick={()=>category=tab}>{tab==='all'?t('All mail'):tab.charAt(0).toUpperCase()+tab.slice(1)}</button>{/each}</div>
 <div class="list-meta"><span>{visible.length} {t("conversations")}</span><span>{t("Newest first ↓")}</span></div>
 <div class="rows" use:restoreScroll onscroll={e=>listScroll=e.currentTarget.scrollTop}>{#each visible as m}<button class="message" class:selected={selected===m.id} onclick={()=>open(m)}><div class="row-top"><strong>{m.sender}</strong><time>{m.date}</time></div><div class="row-subject">{#if m.unread}<span class="unread"></span>{/if}{m.subject}</div><p>{m.body.replaceAll('\n',' ')}</p><div class="row-footer"><span class:work={m.account===t('Work')}>{m.account}</span>{#if m.starred}<Star size={13} fill="currentColor"/>{/if}</div></button>{/each}{#if !visible.length}<div class="empty"><Check size={28}/><h2>{t("A little breathing room.")}</h2><p>{t("No conversations in this view.")}</p></div>{/if}</div>
 <div class="list-bottom"><span class="status-dot"></span> {desktop?t('Encrypted local demo'):t('Browser demo')} · {messages.length} {t("sample messages")}</div></section>{/if}
 <section class="reader">
 {#if current}<div class="reader-toolbar"><div><button title={t("Archive (E)")} onclick={()=>mutate('archive')}><Archive size={18}/></button><button title={t("Snooze demo")} onclick={()=>mutate('snoozed')}><Clock size={18}/></button><button title={t("Move to trash")} onclick={()=>mutate('trash')}><Trash2 size={18}/></button><span class="toolbar-divider"></span><button title={t("Toggle star")} onclick={()=>{if(current){current.starred=!current.starred;persist()}}}><Star size={18} fill={current.starred?'currentColor':'none'}/></button></div><button class="focus-button" onclick={()=>focus=!focus}>{focus?t('Exit focus'):t('Focus mode')}<ArrowUpRight size={14}/></button></div>
 <article><div class="reader-eyebrow"><span class="status-dot"></span> {current.account} <span> / </span> {current.category}</div><h2>{current.subject}</h2><div class="sender"><div class="sender-avatar">{current.sender.split(' ').map(s=>s[0]).slice(0,2).join('')}</div><div><strong>{current.sender}</strong><span>&lt;{current.address}&gt;</span><small>{t("to me")} <ChevronRight size={10}/></small></div><time>{current.date}</time></div>
 <div class="body">{current.body}</div>
 {#if ai}<div class="ai-result"><span><Sparkles size={15}/> {t("AI suggestion · verify against the message")}</span><p>{ai}</p></div>{/if}
 <div class="reply-actions"><button class="reply" onclick={reply}><ArrowLeft size={16}/> {t("Reply")}</button><button class="ai-button" onclick={summarize} disabled={busy}><Sparkles size={15}/>{busy?'Thinking…':t('Summarize')}</button></div><div class="privacy-note">{t("Remote content is blocked. Your attention stays yours.")}</div></article>
 <footer><span><kbd>J</kbd> <kbd>K</kbd> {t("navigate")}</span><span><kbd>E</kbd> {t("archive")}</span><span><kbd>R</kbd> {t("reply")}</span><span class="footer-right">{t("One conversation at a time.")}</span></footer>
 {:else}<div class="empty reader-empty"><Mail size={36}/><h2>{t("Make a little space.")}</h2><p>{t("Select a conversation when you’re ready.")}</p></div>{/if}
 </section></div>
 </main>
 {#if notice}<div class="toast" role="status">{notice}{#if last}<button onclick={undo}>{t("Undo")}</button>{/if}<button aria-label={t("Dismiss")} onclick={()=>notice=''}><X size={15}/></button></div>{/if}
 {#if error}<div class="toast error" role="alert">{error}<button aria-label={t("Dismiss error")} onclick={()=>error=''}><X size={15}/></button></div>{/if}
 {#if compose||settings||palette}<div class="overlay" role="presentation"><div class="modal" use:modalFocus role="dialog" aria-modal="true" aria-label={compose?t('Compose'):settings?t('Settings'):t('Commands')} tabindex="-1"><div class="modal-header"><h2>{compose?t('A new conversation'):settings?t('Your preferences'):t('What would you like to do?')}</h2><button aria-label={t("Close")} onclick={()=>{compose=false;settings=false;palette=false}}><X size={20}/></button></div>
 {#if compose}<label>{t("To")}<input type="email" bind:value={to}/></label><label>{t("Subject")}<input bind:value={subject}/></label><label>{t("Message")}<textarea rows="12" bind:value={body}></textarea></label><div class="modal-actions"><button class="reply" onclick={draft}>{t("Save draft")}</button><span>{t("Sending is not connected yet.")}</span></div>
 {:else if settings}<LanguagePicker/><p>{t("This inbox contains sample mail. Google and Microsoft browser sign-in is available after OAuth app registration; mail synchronization and sending are not implemented yet.")}</p><button class="setting-row" onclick={()=>dark=!dark}>{t("Dark appearance")} <span>{dark?t('On'):t('Off')}</span></button><p>{t("AI runs only when requested. Configure a loopback OpenAI-compatible server with")} <code>RUSTYMAIL_LLM_BASE_URL</code> {t("and")} <code>RUSTYMAIL_LLM_MODEL</code>{t(". Hosted endpoints remain disabled until consent support is implemented.")}</p><label>{t("Reading text size:")} {fontSize}px<input type="range" min="14" max="22" bind:value={fontSize}/></label><label>{t("Reading width:")} {lineWidth}px<input type="range" min="580" max="1000" step="20" bind:value={lineWidth}/></label><button class="setting-row" onclick={()=>compact=!compact}>{t("Compact message list")} <span>{compact?t("On"):t("Off")}</span></button><p>{t("Notifications are off. Desktop sample mail and drafts use encrypted storage.")}</p><AccountSetup/>
 {:else}<label class="search"><Search size={18}/><input placeholder={t("Search your mail…")} bind:value={query}/></label>{#each [t('Compose a message'),t('Toggle focus mode'),t('Open settings'),t('Go to inbox')] as command,i}<button class="command-row" onclick={()=>{palette=false;if(i===0)compose=true;if(i===1)focus=!focus;if(i===2)settings=true;if(i===3){view='inbox';category='all';query=''}}}>{command}<ChevronRight size={16}/></button>{/each}{/if}
 </div></div>{/if}
</div>
{/if}

