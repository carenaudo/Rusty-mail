<script lang="ts">
 import {dateLocale,folderName,t} from './locale.svelte';
 import LanguagePicker from './LanguagePicker.svelte';
 import {onMount,tick} from 'svelte';
 import {invoke} from '@tauri-apps/api/core';
 import {listen} from '@tauri-apps/api/event';
 import {Mail,Inbox,Star,Clock,Archive,Trash2,PenLine,Search,Settings,RefreshCw,X,Paperclip,ListTodo,FileText,Folder,Tag,ArrowLeft,Sparkles,ChevronRight,Send,Sun,Command} from 'lucide-svelte';
 import AccountSetup from './AccountSetup.svelte';
 import MailBody from './MailBody.svelte';
 import SecurityConfirmation from './SecurityConfirmation.svelte';
 import RecipientField, {type RecentRecipient} from './RecipientField.svelte';
 type Account={id:string;provider:string;email:string};
 type File={id:string;name:string;size:number;mime:string};
 type Mailbox={id:string;account_id:string;name:string;label:boolean};
 type Message={id:string;account_id:string;remote_id:string;provider:string;thread_id:string;conversation_key:string;conversation_count:number;message_id:string;headers_shortened:boolean;account:string;sender:string;address:string;subject:string;body:string;html_body:string;to?:string[];cc?:string[];reply_to?:string[];category:string;unread:boolean;starred:boolean;folder:string;date:string;loaded:boolean;attachments:File[];labels?:string[]};
 type Job={id:string;account_id:string;kind:string;status:string;detail:string;due:number};
 type Info={configured:boolean;endpoint:string;model:string;hosted:boolean;consent:boolean};
 type Cursor={received:number;id:string};
 type MailFilters={unread:boolean;sender:string;received_from:number|null;received_before:number|null};
 type PageRequest={version:number;scope:ReturnType<typeof mailScope>;cursor:Cursor|null;draftOffset:number};
 type DraftSummary={id:string;account_id:string;subject:string;to:string;updated:number};
 type DraftPage={drafts:DraftSummary[];next_offset:number|null};
 type DraftData={
  account_id:string;to:string;cc:string;bcc:string;subject:string;body:string;
  signature?:string;attachments:string[];reply_message_id:string|null;source_message_id:string|null;mode:string
 };
 type BulkAction='archive'|'trash'|'read';
 type ActiveJob=Job&{subject:string|null};
 type ActiveJobPage={jobs:ActiveJob[];next_offset:number|null};
 const bulkActions: {action:BulkAction;label:string}[]=[
  {action:'archive',label:'Archive selected'},{action:'trash',label:'Trash selected'},{action:'read',label:'Mark selected as read'}
 ];
 const activeStatus=(job:Job)=>['pending','running','uncertain','failed'].includes(job.status);
 let unreadOnly=$state(false),senderFilter=$state(''),receivedFrom=$state(''),receivedThrough=$state('');
 let filterSummary=$derived([
  unreadOnly?t('Unread only'):'',senderFilter.trim()?t('From {sender}',{sender:senderFilter.trim()}):'',
  receivedFrom?t('From date {date}',{date:receivedFrom}):'',receivedThrough?t('Through {date}',{date:receivedThrough}):''
 ].filter(Boolean));
 let filterError=$derived(receivedFrom&&receivedThrough&&receivedFrom>receivedThrough?t('Start date must be on or before the end date.'):'');
 let queryPending=$state(false),queryTimer:ReturnType<typeof setTimeout>|undefined;
 let checkedRows=$state<string[]>([]),bulkBusy=$state(false),messageActionBusy=$state(false),readerMenu=$state<HTMLDetailsElement>();
 let operations=$state(false),operationOffset=$state(0),operationsBusy=$state(false),operationVersion=0;
 let activePage=$state<ActiveJobPage>({jobs:[],next_offset:null});
 let pageCursor:Cursor|null=$state(null),nextCursor:Cursor|null=$state(null),cursorStack:(Cursor|null)[]=$state([]),pageBusy=$state(false);
 let accounts:Account[]=$state([]),messages:Message[]=$state([]),jobs:Job[]=$state([]),current:Message|null=$state(null),account=$state(''),folder=$state('inbox'),search=$state(''),total=$state(0),busy=$state(false),error=$state(''),notice=$state(''),focus=$state(false),dark=$state(window.matchMedia('(prefers-color-scheme: dark)').matches),settings=$state(false),compose=$state(false),palette=$state(false),status=$state(''),ai=$state(''),aiBusy=$state(false),consent:Info|null=$state(null),consentAccount=$state(''),aiTask=$state('summary');
 type NotificationPreferences={enabled:boolean;previews:boolean};
 type NotificationArrival={id:string;account:string};
 let unreadCounts=$state<Record<string,number>>({});
 let notificationPreferences=$state<NotificationPreferences|null>(null);
 let notificationSaving=$state(false);
 let unreadVersion=0;
 let notifiedMessageId='';
 let inboxUnread=$derived(account?unreadCounts[account]||0:Object.values(unreadCounts).reduce((sum,n)=>sum+n,0));
 async function refreshUnread(){
  const version=++unreadVersion;
  try{
   const counts=await invoke<Record<string,number>>('unread_counts');
   if(version===unreadVersion)unreadCounts=counts;
  }catch(e){if(version===unreadVersion)error=String(e)}
 }
 $effect(()=>{
  if(settings)invoke<NotificationPreferences>('notification_preferences')
   .then(value=>notificationPreferences=value).catch(e=>error=String(e));
 });
 async function saveNotificationPreferences(value:NotificationPreferences){
  notificationSaving=true;
  try{notificationPreferences=await invoke<NotificationPreferences>('set_notification_preferences',{value})}
  catch(e){error=String(e)}
  finally{notificationSaving=false}
 }
 async function openNotification(arrival:NotificationArrival){
  const version=++openVersion;
  try{
   const members=await invoke<Message[]>('get_conversation',{id:arrival.id});
   if(version!==openVersion)return;
   const message=members.find(member=>member.id===arrival.id&&member.account_id===arrival.account);
   if(!message){notice=t('This message is no longer available.');return}
   account=arrival.account;folder='inbox';search='';focus=false;
   clearFilters();
   notifiedMessageId=arrival.id;
   selectedId=arrival.id;
   conversation=members;
   await expand(message);
  }catch(e){if(version===openVersion)notice=t('This message is no longer available.')}
 }
 let draftPage=$state<DraftPage>({drafts:[],next_offset:null}),draftOffset=$state(0);
 let mailboxes=$state<Mailbox[]>([]);
 let mailboxGroups=$derived(accounts.filter(a=>!account||a.id===account).map(a=>({
  account:a,mailboxes:mailboxes.filter(box=>box.account_id===a.id)
 })).filter(group=>group.mailboxes.length));
 let readerMailboxes=$derived(mailboxes.filter(box=>box.account_id===current?.account_id));
 let mailboxTitle=$derived(folder==='inbox'?t('All inbox'):folder==='drafts'?t('Drafts'):
  mailboxes.find(box=>box.account_id===account&&folder==='mailbox:'+box.id)?.name||folderName(folder));
 let catalogVersion=0;
 let allRowsChecked=$derived(messages.length>0&&checkedRows.length===messages.length);
 let conversation:Message[]=$state([]),selectedId=$state(""),openVersion=0,loadError=$state('');
 let updateOffer:{enabled:boolean;version:string|null;notes:string|null}|null=$state(null);let updateBusy=$state(false);
 let from=$state(''),to=$state(''),cc=$state(''),bcc=$state(''),sourceId:string|null=$state(null),composeMode=$state('new'),forwardChoices:File[]=$state([]),forwardSelected:string[]=$state([]),composeBusy=$state(false),aliasValues:Record<string,string>=$state({}),subject=$state(''),body=$state(''),files:File[]=$state([]),replyId:string|null=$state(null),fontSize=$state(16),lineWidth=$state(840),compact=$state(false),queryVersion=0;
 async function refreshAccounts(){accounts=await invoke<Account[]>('list_accounts');await loadSignatures();if(!from&&accounts.length)from=accounts[0].id;void refreshUnread();await page();await refreshMailboxes();if(accounts.length&&accounts.every(a=>a.provider==='synthetic')){await tick();await new Promise<void>(r=>requestAnimationFrame(()=>requestAnimationFrame(()=>r())));await invoke('synthetic_profile_ready',{rows:messages.length})}}
 function localDateBoundary(value:string,followingDay=false){
  if(!value)return null;
  const date=new Date(value+'T00:00:00');
  if(followingDay)date.setDate(date.getDate()+1);
  return date.getTime()/1000;
 }
 function mailScope(){
  const filters:MailFilters|null=filterSummary.length?{
   unread:unreadOnly,sender:senderFilter.trim(),
   received_from:localDateBoundary(receivedFrom),
   received_before:localDateBoundary(receivedThrough,true)
  }:null;
  return {folder,account,search,filters};
 }
 let pendingPage:PageRequest|undefined;
 let pageTask:Promise<void>|undefined;
 async function page(){
  if(queryPending||(folder!=='drafts'&&filterError))return;
  pendingPage={version:++queryVersion,scope:mailScope(),cursor:pageCursor,draftOffset};
  pageBusy=true;
  pageTask??=Promise.resolve().then(async()=>{
   while(pendingPage){
    const request=pendingPage;
    pendingPage=undefined;
    await readPage(request);
   }
   pageTask=undefined;
  });
  await pageTask;
 }
 async function readPage({version,scope,cursor,draftOffset:requestedOffset}:PageRequest){
  if(version!==queryVersion)return;
  try{
   if(scope.folder==='drafts'){
    let offset=requestedOffset;
    let result=await invoke<DraftPage>('list_live_drafts',{accountId:scope.account,offset});
    if(version!==queryVersion)return;
    if(!result.drafts.length&&offset>0){
     offset=0;
     result=await invoke<DraftPage>('list_live_drafts',{accountId:scope.account,offset});
    }
    if(version===queryVersion){draftPage=result;draftOffset=offset}
    return;
   }
   const p=await invoke<{messages:Message[];total:number;next_cursor:Cursor|null}>('query_mail',{...scope,offset:0,cursor});
   if(version!==queryVersion)return;
   messages=p.messages;
   total=p.total;
   nextCursor=p.next_cursor;
   const visibleIds=new Set(messages.map(m=>m.id));
   checkedRows=checkedRows.filter(id=>visibleIds.has(id));
   if(current&&current.id!==notifiedMessageId&&!messages.some(m=>sameConversation(m,current!))){
    current=null;conversation=[];openVersion++;
   }else if(current){
    const row=messages.find(m=>sameConversation(m,current!));
    if(row)selectedId=row.id;
    await reloadConversation(current.id);
   }
  }catch(e){if(version===queryVersion)error=String(e)}
  finally{if(version===queryVersion)pageBusy=false}
 }
 function resetPagination(){
  pageCursor=null;nextCursor=null;cursorStack=[];checkedRows=[];
  listScroll=0;
  if(rowsElement)rowsElement.scrollTop=0;
 }
 function changeScope(debounce=false){
  clearTimeout(queryTimer);
  notifiedMessageId='';
  queryVersion++;
  pendingPage=undefined;
  queryPending=debounce;
  pageBusy=false;
  resetPagination();
  draftOffset=0;draftPage={drafts:[],next_offset:null};
  if(debounce){
   queryTimer=setTimeout(()=>{queryPending=false;page()},250);
  }else{
   page();
  }
 }
 function clearFilters(){
  unreadOnly=false;senderFilter='';receivedFrom='';receivedThrough='';
  changeScope();
 }
 function nextPage(){
  if(pageBusy||!nextCursor)return;
  checkedRows=[];
  cursorStack=[...cursorStack,pageCursor];pageCursor=nextCursor;page();
 }
 function previousPage(){
  if(pageBusy||!cursorStack.length)return;
  checkedRows=[];
  pageCursor=cursorStack.at(-1)??null;cursorStack=cursorStack.slice(0,-1);page();
 }
 function toggleRow(id:string,checked:boolean){
  checkedRows=checked?[...checkedRows,id]:checkedRows.filter(row=>row!==id);
 }
 async function bulkAction(action:BulkAction){
  if(!checkedRows.length||bulkBusy||pageBusy||queryPending||filterError)return;
  if(bulkMenu?.open){bulkMenu.open=false;bulkMenu.querySelector('summary')?.focus()}
  bulkBusy=true;error='';
  try{
   const count=await invoke<number>('queue_bulk_mutation',{scope:mailScope(),rowIds:[...checkedRows],action});
   clearSelection();
   notice=count===0?t('The selected messages already match this action.'):t(count===1?'{count} message queued.':'{count} messages queued.',{count});
   await refreshJobs();
  }catch(e){
   error=String(e);
   if(error.includes('The selection changed'))checkedRows=[];
  }finally{void refreshUnread();await page();bulkBusy=false}
 }
 async function refreshMailboxes(){
  const version=++catalogVersion;
  const updated=await invoke<Mailbox[]>('list_mailboxes');
  if(version!==catalogVersion)return;
  mailboxes=updated;
  if(folder.startsWith('mailbox:')&&!mailboxes.some(box=>box.account_id===account&&folder==='mailbox:'+box.id)){
   navigate('inbox');
  }
 }
 function browseMailbox(box:Mailbox){
  account=box.account_id;
  navigate('mailbox:'+box.id);
 }
 async function organize(box:Mailbox,enabled:boolean|null){
  if(!current?.loaded||messageActionBusy)return;
  const id=current.id;
  messageActionBusy=true;error='';
  try{
   await invoke('organize_mail',{id,target:box.id,enabled});
   if(current?.id===id){
    if(box.label){
     const labels=(current.labels||[]).filter(label=>label!==box.id);
     current.labels=enabled?[...labels,box.id]:labels;
    }else{
     current=null;conversation=[];openVersion++;
    }
   }
   notice=box.label?(enabled?t('Label applied.'):t('Label removed.')):t('Moved to {name}.',{name:box.name});
   void refreshUnread();
   await page();
  }catch(e){error=String(e)}
  finally{messageActionBusy=false}
 }
 async function refresh(){busy=true;error='';try{for(const a of accounts.filter(a=>!account||a.id===account)){status=t('Synchronizing {email}…',{email:a.email});await invoke('sync_account',{id:a.id})}status=t('Synchronization finished')}catch(e){error=String(e);status=t('Sync needs attention')}finally{void refreshUnread();await Promise.all([page(),refreshMailboxes().catch(e=>error=String(e))]);busy=false}}
 function navigate(next:string){closeNavigation(true);folder=next;current=null;conversation=[];openVersion++;changeScope()}
 function selectAccount(id:string){
  closeNavigation(true);
  if(account!==id&&folder.startsWith('mailbox:')){account=id;navigate('inbox');return}
  account=id;changeScope();
 }
 function sameConversation(a:Message,b:Message){return a.id===b.id||!!a.conversation_key&&a.account_id===b.account_id&&a.conversation_key===b.conversation_key}
 async function reloadConversation(id:string){const version=openVersion;const members=await invoke<Message[]>('get_conversation',{id});if(version===openVersion&&current&&members.some(m=>m.id===current?.id))conversation=members}
 async function expand(m:Message){
  const version=++openVersion;
  if(readerMenu)readerMenu.open=false;
  current={...m,loaded:false};loadError='';error='';ai='';
  try{
   const full=await invoke<Message>('get_mail',{id:m.id});
   if(version!==openVersion)return;
   if(full.unread){
    try{
     await invoke('queue_mutation',{id:m.id,action:'read'});
     if(version!==openVersion)return;
     full.unread=false;
     void refreshUnread();
     void page();
     conversation=conversation.map(x=>x.id===m.id?{...x,unread:false}:x);
    }catch(e){if(version===openVersion)error=String(e)}
   }
   if(version===openVersion)current=full;
  }catch(e){if(version===openVersion){error=String(e);loadError=t('Could not load this message.')}}
 }
 async function open(m:Message){notifiedMessageId='';selectedId=m.id;conversation=[m];await expand(m);if(current&&sameConversation(m,current))try{await reloadConversation(m.id)}catch(e){error=String(e)}}
 async function action(name:string){
  if(!current||messageActionBusy)return;
  const id=current.id;
  if(readerMenu?.open){
   readerMenu.open=false;
   readerMenu.querySelector('summary')?.focus();
  }
  messageActionBusy=true;
  try{
   await invoke('queue_mutation',{id,action:name});
   if(name!=='star'&&name!=='unstar')void refreshUnread();
   notice=t('Saved. Provider operation is queued.');
   if(current?.id===id){
    if(name==='unread'){
     current.unread=true;
     conversation=conversation.map(m=>m.id===id?{...m,unread:true}:m);
    }else if(name==='star'||name==='unstar'){
     current.starred=name==='star';
    }else{
     const next=['archive','trash'].includes(name)?messages[messages.findIndex(m=>m.id===selectedId)+1]:null;
     current=null;conversation=[];openVersion++;
     const version=openVersion;
     await page();
     if(next&&version===openVersion&&messages.some(m=>m.id===next.id))open(next);
     await refreshJobs();
     return;
    }
   }
   await page();
   await refreshJobs();
  }catch(e){error=String(e)}
  finally{messageActionBusy=false}
 }
 let snoozeTimer:ReturnType<typeof setTimeout>|undefined;
 let snoozeVersion=0;
 async function scheduleSnooze(){
  const version=++snoozeVersion;
  clearTimeout(snoozeTimer);
  try{
   const wake=await invoke<number|null>('next_snooze_wake');
   if(version!==snoozeVersion||wake===null)return;
   const delay=wake*1000-Date.now();
   snoozeTimer=setTimeout(()=>{
    if(version!==snoozeVersion)return;
    if(Date.now()>=wake*1000){void page();void refreshUnread()}
    void scheduleSnooze();
   },Math.min(Math.max(delay,0),2_147_483_647));
  }catch(e){if(version===snoozeVersion)error=String(e)}
 }
 async function snooze(){if(!current)return;const id=current.id;try{await invoke('snooze_mail',{id,wake:Math.floor(Date.now()/1000)+86400});notice=t('Snoozed until tomorrow on this device');if(current?.id===id){current=null;conversation=[];openVersion++}void scheduleSnooze();void refreshUnread();await page()}catch(e){error=String(e)}}
 let jobsTask:Promise<void>|undefined;
 let jobsPending=false,requestedOperationOffset=0;
 function refreshJobs(){
  jobsPending=true;
  jobsTask??=Promise.resolve().then(async()=>{
   try{
    while(jobsPending){
     jobsPending=false;
     jobs=await invoke<Job[]>('list_jobs');
     if(operations)await loadOperations(requestedOperationOffset);
    }
   }finally{jobsTask=undefined}
  });
  return jobsTask;
 }
 async function loadOperations(offset:number){
  requestedOperationOffset=offset;
  const version=++operationVersion;
  operationsBusy=true;
  try{
   let result=await invoke<ActiveJobPage>('list_active_jobs',{offset});
   // Jobs can finish while the dialog is open and empty its last page.
   if(!result.jobs.length&&offset>0){
    offset=0;
    result=await invoke<ActiveJobPage>('list_active_jobs',{offset});
   }
   if(version!==operationVersion)return;
   activePage=result;operationOffset=offset;requestedOperationOffset=offset;
  }catch(e){if(version===operationVersion)error=String(e)}
  finally{if(version===operationVersion)operationsBusy=false}
 }
 function openOperations(){operations=true;activePage={jobs:[],next_offset:null};loadOperations(0)}
 async function manageJob(job:Job,command:string){
  try{
   await invoke(command,{id:job.id});
   await refreshJobs();
   if(command==='cancel_job'){void refreshUnread();await page();notice=t('Cancelled before submission')}
  }catch(e){error=String(e)}
 }
 let accountSignatures=$state<Record<string,string>>({});
 let signatureValues=$state<Record<string,string>>({});
 let signatureSaving=$state(false),signature=$state(''),showSignature=$state(false);
 let recentRecipients=$state<RecentRecipient[]>([]);
 async function loadSignatures(){
  const values=await invoke<Record<string,string>>('get_signatures');
  accountSignatures=values;
  signatureValues={...values};
 }
 async function saveSignature(accountId:string){
  signatureSaving=true;
  try{
   const value=await invoke<string>('set_signature',{accountId,signature:signatureValues[accountId]||''});
   accountSignatures[accountId]=value;
   signatureValues[accountId]=value;
   notice=value?t('Signature saved.'):t('Signature cleared.');
  }catch(e){error=String(e)}
  finally{signatureSaving=false}
 }
 $effect(()=>{
  if(!compose||!from)return;
  const accountId=from;
  let cancelled=false;
  recentRecipients=[];
  invoke<RecentRecipient[]>('recent_recipients',{accountId}).then(recipients=>{
   if(!cancelled)recentRecipients=recipients;
  }).catch(e=>{if(!cancelled)error=String(e)});
  return()=>{cancelled=true};
 });
 let navigationOpen=$state(false),navigationButton=$state<HTMLButtonElement>(),bulkMenu=$state<HTMLDetailsElement>();
 let showCc=$state(false),showBcc=$state(false),composeError=$state('');
 let composerFocus:'to'|'start'|'end'='to';
 let composerOpener:HTMLElement|null=null;
 let draftId='',draftExists=false;
 async function revealRecipient(field:'cc'|'bcc'){
  if(field==='cc')showCc=true;
  else showBcc=true;
  await tick();
  document.querySelector<HTMLInputElement>('.compose-dialog [data-recipient="'+field+'"] input')?.focus();
 }
 function closeNavigation(restore=false){
  if(!navigationOpen)return;
  navigationOpen=false;
  if(restore)navigationButton?.focus();
 }
 async function toggleNavigation(){
  if(navigationOpen){closeNavigation(true);return}
  navigationOpen=true;
  await tick();
  document.querySelector<HTMLButtonElement>('#mail-navigation .compose')?.focus();
 }
 function clearSelection(){
  if(bulkMenu){
   bulkMenu.open=false;
   if(bulkMenu.contains(document.activeElement)||document.activeElement?.classList.contains('clear-selection')){
    bulkMenu.closest('.triage-controls')?.querySelector<HTMLInputElement>('input')?.focus();
   }
  }
  checkedRows=[];
 }
 let draftFinishing=$state(false),confirmDiscard=$state(false);
 let draftTimer:ReturnType<typeof setTimeout>|undefined,draftSave:Promise<unknown>=Promise.resolve();
 let composeVersion=0;
 function draftData():DraftData{
  return {account_id:from,to,subject,body,attachments:files.map(f=>f.id),signature,reply_message_id:replyId,cc,bcc,source_message_id:sourceId,mode:composeMode};
 }
 function persistDraft(draft=draftData()){
  if(!draftExists&&!draft.attachments.length&&![draft.to,draft.cc,draft.bcc,draft.subject,draft.body].some(value=>value.trim())){
   return draftSave;
  }
  const id=draftId;
  draftExists=true;
  draftSave=draftSave.catch(()=>{}).then(()=>invoke('save_live_draft',{id,draft}));
  return draftSave;
 }
 async function finishDraft(operation:'save'|'send'|'discard'){
  if(!compose||draftFinishing)return;
  draftFinishing=true;
  composeError='';
  composeVersion++;
  clearTimeout(draftTimer);
  const id=draftId;
  const draft=draftData();
  try{
   if(operation==='discard'){
    await draftSave.catch(()=>{});
    if(draftExists)await invoke('discard_live_draft',{id});
    notice=t('Draft discarded.');
   }else{
    await persistDraft(draft);
    if(operation==='send'){
     await invoke('queue_send',{draft,draftId:id});
     notice=t('Queued. You have 10 seconds to cancel before submission.');
    }
   }
   compose=false;
   confirmDiscard=false;
   if(folder==='drafts')await page();
   if(operation==='send')await refreshJobs();
  }catch(e){composeError=String(e);await tick();document.querySelector<HTMLElement>('.compose-error')?.focus()}
  finally{draftFinishing=false}
 }
 async function closeModal(){
  if(compose){
   if(confirmDiscard){confirmDiscard=false;return}
   await finishDraft('save');
   return;
  }
  composeVersion++;
  settings=false;palette=false;consent=null;operations=false;operationsBusy=false;operationVersion++;
 }
 function resetSource(){
  composeVersion++;replyId=null;sourceId=null;composeMode='new';forwardChoices=[];forwardSelected=[];
  signature=accountSignatures[from]||'';showSignature=!!signature;
  if(files.length){files=[];notice=t('Attachments cleared because the sending account changed')}
 }
 function openComposer(id:string,draft:DraftData,attachments:File[],originalFiles:File[],saved:boolean){
  composeError='';
  showCc=!!draft.cc?.trim();showBcc=!!draft.bcc?.trim();
  composerFocus=saved?(draft.to.trim()?'end':'to'):['reply','reply_all'].includes(draft.mode)?'start':'to';
  draftId=id;draftExists=saved;draftSave=Promise.resolve();confirmDiscard=false;
  from=draft.account_id;to=draft.to;cc=draft.cc||'';bcc=draft.bcc||'';subject=draft.subject;body=draft.body;
  replyId=draft.reply_message_id;sourceId=draft.source_message_id||null;composeMode=draft.mode||'new';
  signature=saved?draft.signature||'':accountSignatures[from]||'';showSignature=!!signature;
  files=attachments;forwardChoices=originalFiles;forwardSelected=[];
  compose=true;
 }
 async function newDraft(mode='new'){
  if(compose||composeBusy||draftFinishing)return;
  composerOpener=document.activeElement instanceof HTMLElement?document.activeElement:null;
  if(!accounts.length){settings=true;return}
  const id=crypto.randomUUID().replaceAll('-','');
  if(mode==='new'){
   const accountId=account||from||accounts[0].id;
   openComposer(id,{account_id:accountId,to:'',cc:'',bcc:'',subject:'',body:'',attachments:[],
    reply_message_id:null,source_message_id:null,mode},[],[],false);
   return;
  }
  if(!current?.loaded)return;
  composeBusy=true;
  const original=current.id,version=++composeVersion;
  try{
   const seed=await invoke<{draft:DraftData;available_attachments:File[]}>('prepare_compose',{id:original,mode});
   if(current?.id===original&&version===composeVersion)openComposer(id,seed.draft,[],seed.available_attachments,false);
  }catch(e){error=String(e)}
  finally{composeBusy=false}
 }
 async function reopenDraft(id:string){
  if(compose||composeBusy||draftFinishing)return;
  composerOpener=document.activeElement instanceof HTMLElement?document.activeElement:null;
  composeBusy=true;
  const version=++composeVersion;
  try{
   const draft=await invoke<DraftData>('load_live_draft',{id});
   const attachments=draft.attachments.length
    ?await invoke<File[]>('outgoing_attachments',{accountId:draft.account_id,ids:draft.attachments}):[];
   if(version===composeVersion)openComposer(id,draft,attachments,[],true);
  }catch(e){error=String(e)}
  finally{composeBusy=false}
 }
 async function includeForwardFiles(){if(!sourceId||!forwardSelected.length||composeBusy)return;composeBusy=true;const version=composeVersion;const originalAccount=from;const originalSource=sourceId;try{const added=await invoke<File[]>('copy_forward_attachments',{messageId:originalSource,accountId:originalAccount,ids:forwardSelected});if(!compose||version!==composeVersion||from!==originalAccount||sourceId!==originalSource)return;files=[...files,...added];forwardChoices=forwardChoices.filter(f=>!forwardSelected.includes(f.id));forwardSelected=[]}catch(e){error=String(e)}finally{composeBusy=false}}
 $effect(()=>{if(settings)invoke<Record<string,string>>('get_account_aliases').then(v=>aliasValues=v).catch(e=>error=String(e))});
 async function saveAliases(accountId:string){try{await invoke('save_account_aliases',{accountId,addresses:aliasValues[accountId]||''});notice=t('Your addresses are saved for Reply all exclusions')}catch(e){error=String(e)}}
 $effect(()=>{
  if(!compose||!from||draftFinishing)return;
  const draft=draftData();
  draftTimer=setTimeout(()=>persistDraft(draft).catch(e=>error=String(e)),800);
  return()=>clearTimeout(draftTimer);
 });
 async function attach(){
  const accountId=from,version=composeVersion;
  try{
   const added=await invoke<File[]>('pick_attachments',{accountId});
   if(compose&&version===composeVersion&&from===accountId)files=[...files,...added];
  }catch(e){error=String(e)}
 }
 let pendingAttachment=$state.raw<{messageId:string;file:File}|null>(null);
 $effect(()=>{
  if(pendingAttachment&&(!current?.loaded||current.id!==pendingAttachment.messageId))pendingAttachment=null;
 });
 function requestAttachment(file:File){
  if(current?.loaded)pendingAttachment={messageId:current.id,file};
 }
 async function confirmAttachment(){
  const pending=pendingAttachment;
  pendingAttachment=null;
  if(!pending||!current?.loaded||pending.messageId!==current.id)return;
  try{
   const saved=await invoke<boolean>('save_attachment',{messageId:pending.messageId,id:pending.file.id});
   if(saved)notice=t('Attachment saved');
  }catch(e){error=String(e)}
 }
 async function ask(task='summary'){if(!current)return;aiTask=task;try{const info=await invoke<Info>('ai_info',{accountId:current.account_id});if(!info.configured){error=t('Configure a local or hosted model using RUSTYMAIL_LLM_BASE_URL and RUSTYMAIL_LLM_MODEL');return}if(info.hosted&&!info.consent){consent=info;consentAccount=current.account_id;return}await runAi(task)}catch(e){error=String(e)}}
 async function runAi(task:string){if(!current)return;aiBusy=true;const id=current.id;ai='';try{const text=await invoke<string>('run_ai',{id,task});if(current?.id===id)ai=text}catch(e){error=String(e)}finally{aiBusy=false}}
 async function approve(){try{await invoke('ai_consent',{accountId:consentAccount,allowed:true});consent=null;if(current?.account_id===consentAccount)await runAi(aiTask)}catch(e){error=String(e)}}
 async function revokeAi(){if(!current)return;try{await invoke('ai_consent',{accountId:current.account_id,allowed:false});notice=t('Hosted AI consent removed for this account and destination')}catch(e){error=String(e)}}
 function keyboard(e:KeyboardEvent){if(e.defaultPrevented||(e.target instanceof Element&&e.target.closest('dialog.security-confirmation')))return;if(e.key==='Escape'){if(compose||settings||palette||consent||operations){closeModal();return}if(navigationOpen){closeNavigation(true);return}if(bulkMenu?.open){bulkMenu.open=false;bulkMenu.querySelector('summary')?.focus();return}if(readerMenu?.open){readerMenu.open=false;readerMenu.querySelector('summary')?.focus();return}closeModal();focus=false;return}if((e.ctrlKey||e.metaKey)&&e.key==='k'&&!operations){e.preventDefault();palette=!palette;return}if(e.ctrlKey||e.metaKey||e.altKey||settings||compose||palette||consent||operations||(e.target instanceof Element&&e.target.closest('input,textarea,select')))return;if(e.key==='e')action('archive');if(e.key==='r')newDraft('reply');if(e.key==='c')newDraft();if(e.key==='j'||e.key==='k'){const i=messages.findIndex(m=>m.id===selectedId);const next=messages[i+(e.key==='j'?1:-1)];if(next)open(next)}}
 function modal(node:HTMLElement){
  const previous=compose?composerOpener:document.activeElement;
  const fields=()=>Array.from(node.querySelectorAll<HTMLElement>('button,input,textarea,select,summary,[tabindex]'))
   .filter(element=>element.tabIndex>=0&&!element.matches(':disabled')&&element.getClientRects().length>0);
  queueMicrotask(()=>{
   if(!node.isConnected)return;
   if(!compose){fields()[0]?.focus();return}
   if(composerFocus==='to'){
    node.querySelector<HTMLInputElement>('[data-recipient="to"] input')?.focus({preventScroll:true});
   }else{
    const editor=node.querySelector<HTMLTextAreaElement>('.message-editor');
    if(editor){
     editor.focus({preventScroll:true});
     const caret=composerFocus==='start'?0:editor.value.length;
     editor.setSelectionRange(caret,caret);
    }
   }
  });
  function trap(event:KeyboardEvent){
   if(event.key!=='Tab')return;
   const focusable=fields();
   const position=focusable.indexOf(document.activeElement as HTMLElement);
   if(position<0||(event.shiftKey?position===0:position===focusable.length-1)){
    event.preventDefault();
    (event.shiftKey?focusable.at(-1):focusable[0]||node)?.focus();
   }
  }
  node.addEventListener('keydown',trap);
  return{destroy(){
   node.removeEventListener('keydown',trap);
   queueMicrotask(()=>{
    if(previous instanceof HTMLElement&&previous.isConnected&&previous.getClientRects().length&&!previous.matches(':disabled')){
     previous.focus();
    }else{
     const fallback=window.matchMedia('(max-width: 800px)').matches?navigationButton:document.querySelector<HTMLButtonElement>('.compose');
     fallback?.focus();
    }
   });
  }};
 }
 onMount(()=>{
  let disposed=false;
  let mailTimer:ReturnType<typeof setTimeout>|undefined;
  const unlisteners:(()=>void)[]=[];
  function keep(unlisten:()=>void){if(disposed)unlisten();else unlisteners.push(unlisten)}
  function refreshMail(){
   if(mailTimer!==undefined)return;
   mailTimer=setTimeout(()=>{
    mailTimer=undefined;
    void page();void refreshUnread();
    refreshMailboxes().catch(e=>error=String(e));
   },0);
  }
  function reconcile(){
   refreshMail();
   refreshJobs().catch(e=>error=String(e));
   void scheduleSnooze();
  }
  void Promise.all([
   listen('mail-changed',refreshMail),
   listen<boolean>('jobs-changed',event=>{
    refreshJobs().catch(e=>error=String(e));
    if(event.payload&&folder==='drafts')void page();
   })
  ].map(subscription=>subscription.then(keep).catch(e=>error=String(e)))).then(()=>{
   if(disposed)return;
   refreshAccounts().catch(e=>error=String(e));
   refreshJobs().catch(e=>error=String(e));
   void scheduleSnooze();
   window.addEventListener('focus',reconcile);
  });
  listen<NotificationArrival>('open-mail-notification',event=>openNotification(event.payload))
   .then(keep).catch(e=>error=String(e));
  listen('mail-notification-unavailable',()=>{
   notice=t('Windows could not show a mail notification. Check Windows notification settings and use the installed app.');
  }).then(keep).catch(e=>error=String(e));
  return()=>{
   disposed=true;unlisteners.forEach(unlisten=>unlisten());window.removeEventListener('focus',reconcile);
   clearTimeout(mailTimer);clearTimeout(snoozeTimer);clearTimeout(queryTimer);
   snoozeVersion++;queryVersion++;pendingPage=undefined;
  };
 });
 let listScroll=0,rowsElement:HTMLElement|undefined;
 function restoreRows(node:HTMLElement){rowsElement=node;node.scrollTop=listScroll;return{destroy(){rowsElement=undefined}}}
 const readerDate=$derived(new Intl.DateTimeFormat(dateLocale(),{month:'short',day:'numeric',year:'numeric',hour:'2-digit',minute:'2-digit'}));
 const rowDate=$derived(new Intl.DateTimeFormat(dateLocale(),{month:'short',day:'numeric'}));
 function messageDate(raw:string){const d=new Date(raw);return Number.isNaN(d.getTime())?raw:readerDate.format(d)}
 function date(raw:string){const d=new Date(raw);return Number.isNaN(d.getTime())?raw:rowDate.format(d)}
</script>
<svelte:window onkeydown={keyboard}/>
<div class="app" class:dark class:compact style={`--reader-size:${fontSize}px;--reader-width:${lineWidth}px`}>
 {#if !focus}<aside id="mail-navigation" class:navigation-open={navigationOpen}><div class="brand"><div class="brand-mark"><Mail size={20}/></div>rusty<span>mail</span></div><button class="compose" onclick={()=>newDraft()}><PenLine size={17}/>{t("Compose")}<kbd>C</kbd></button><div class="sidebar-scroll"><nav aria-label={t("Mail folders")}>{#each [{id:'inbox',label:t('All inbox'),icon:Inbox},{id:'starred',label:t('Starred'),icon:Star},{id:'snoozed',label:t('Snoozed'),icon:Clock},{id:'drafts',label:t('Drafts'),icon:FileText},{id:'sent',label:t('Sent'),icon:Send},{id:'archive',label:t('Archive'),icon:Archive},{id:'spam',label:t('Spam'),icon:Mail},{id:'trash',label:t('Trash'),icon:Trash2}] as item}<button class:active={folder===item.id} onclick={()=>navigate(item.id)}><item.icon size={18}/>{item.label}
 {#if item.id==='inbox'&&inboxUnread}<span class="inbox-unread" aria-label={t('{count} unread messages',{count:inboxUnread})}>{inboxUnread}</span>{/if}
 </button>{/each}</nav><div class="section-label account-heading">{t("ACCOUNTS")}<button aria-label={t("Add account")} onclick={()=>settings=true}>＋</button></div><button class="account" onclick={()=>selectAccount('')}>{t("All accounts")}</button>{#each accounts as a}<button class="account" class:active={account===a.id} onclick={()=>selectAccount(a.id)}><span class="account-dot peach">{a.email[0].toUpperCase()}</span><div>{a.email}<small>{a.provider}</small></div>
 {#if unreadCounts[a.id]}<span class="inbox-unread" aria-label={t('{count} unread inbox messages',{count:unreadCounts[a.id]})}>{unreadCounts[a.id]}</span>{/if}
 </button>{/each}<div class="mailbox-groups">
 {#each mailboxGroups as group (group.account.id)}
  <details open={account===group.account.id}>
   <summary>{group.account.provider==='google'?t('Labels'):t('Folders')}{account?'':' · '+group.account.email}</summary>
   <nav aria-label={t('Mailboxes for {email}',{email:group.account.email})}>
    {#each group.mailboxes as box (box.id)}
     <button class:active={account===box.account_id&&folder==='mailbox:'+box.id}
      title={box.name} onclick={()=>browseMailbox(box)}>
      {#if box.label}<Tag size={15}/>{:else}<Folder size={15}/>{/if}<span>{box.name}</span>
     </button>
    {/each}
   </nav>
  </details>
 {/each}
 </div></div><div class="sidebar-bottom"><button onclick={()=>settings=true}><Settings size={17}/>{t("Settings")}</button></div></aside>{/if}
 <main><header><div class="breadcrumb">{#if !focus}<button class="navigation-toggle" bind:this={navigationButton}
  aria-expanded={navigationOpen} aria-controls="mail-navigation" onclick={toggleNavigation}>{t("Mail navigation")}</button>{/if}
  {#if focus}<strong>{mailboxTitle}</strong>{/if}</div><div class="header-actions"><span>{busy?t('Synchronizing…'):t('LIVE MAIL')}</span><button title={t("Sync accounts")} disabled={busy} onclick={refresh}><RefreshCw size={17}/></button><button aria-label={t("Mail operations")} title={t("Mail operations")} onclick={openOperations}><ListTodo size={17}/></button><button title={t("Commands")} onclick={()=>palette=true}><Command size={17}/></button><button title={t("Change theme")} onclick={()=>dark=!dark}><Sun size={18}/></button></div></header><div class="workspace">
 {#if !focus&&folder==='drafts'}
  <section class="message-list" aria-label={t("Drafts")}>
   <div class="list-heading"><div><h1>{t("Drafts")}</h1><p>{t("Saved on this device")}</p></div></div>
   <div class="rows" aria-busy={pageBusy}>
    {#each draftPage.drafts as draft (draft.id)}
     <button class="message draft-row" disabled={composeBusy||pageBusy} onclick={()=>reopenDraft(draft.id)}>
      <div class="row-top"><strong>{draft.subject||t('(No subject)')}</strong><time>{date(new Date(draft.updated).toISOString())}</time></div>
      <p>{draft.to?t('To: {address}',{address:draft.to}):t('No recipients yet')}</p>
      {#if !account&&accounts.length>1}
       <div class="row-footer"><span>{accounts.find(a=>a.id===draft.account_id)?.email||draft.account_id}</span></div>
      {/if}
     </button>
    {:else}
     {#if !pageBusy}<div class="empty"><FileText size={28}/><h2>{t("No saved drafts")}</h2><button class="reply" onclick={()=>newDraft()}>{t("New message")}</button></div>{/if}
    {/each}
   </div>
   {#if draftOffset||draftPage.next_offset!==null}
    <div class="pagination">
     <button disabled={pageBusy||draftOffset===0} onclick={()=>{draftOffset=Math.max(0,draftOffset-50);page()}}>{t("Previous")}</button>
     <span>{t("Page")} {Math.floor(draftOffset/50)+1}</span>
     <button disabled={pageBusy||draftPage.next_offset===null} onclick={()=>{if(draftPage.next_offset!==null){draftOffset=draftPage.next_offset;page()}}}>{t("Next")}</button>
    </div>
   {/if}
  </section>
 {:else if !focus}
  <section class="message-list">
   <div class="list-heading"><div><h1>{mailboxTitle}</h1></div></div>
   <label class="search"><Search size={16}/><input aria-label={t("Search downloaded mail")} value={search} oninput={e=>{search=e.currentTarget.value;changeScope(true)}} placeholder={t("Search downloaded mail")}/></label>
   <details class="mail-filters">
    <summary>{t("Filters")}{filterSummary.length?` (${filterSummary.length})`:''}</summary>
    <div class="filter-fields">
     <label class="unread-filter"><input type="checkbox" checked={unreadOnly} onchange={e=>{unreadOnly=e.currentTarget.checked;changeScope()}}/>{t("Unread only")}</label>
     <label>{t("From email address")}<input type="text" inputmode="email" maxlength="512" value={senderFilter} oninput={e=>{senderFilter=e.currentTarget.value;changeScope(true)}} placeholder="sender@example.com"/></label>
     <div class="filter-dates">
      <label>{t("Received from")}<input type="date" value={receivedFrom} onchange={e=>{receivedFrom=e.currentTarget.value;changeScope()}}/></label>
      <label>{t("Received through")}<input type="date" value={receivedThrough} onchange={e=>{receivedThrough=e.currentTarget.value;changeScope()}}/></label>
     </div>
    </div>
   </details>
   {#if filterSummary.length}
    <div class="filter-summary" aria-label={t("Active filters")}>
     {#each filterSummary as filter}<span>{filter}</span>{/each}
     <button onclick={clearFilters}>{t("Clear filters")}</button>
    </div>
   {/if}
   {#if filterError}<p class="filter-error" role="alert">{filterError}</p>{/if}
   <div class="list-meta"><span>{total} {t("cached conversations")}</span><span>{t("Newest first")}</span></div>
   {#if messages.length}
    <div class="triage-controls">
     <label><input type="checkbox" checked={allRowsChecked} indeterminate={checkedRows.length>0&&!allRowsChecked}
      disabled={pageBusy||queryPending||bulkBusy||!!filterError}
      onchange={e=>checkedRows=e.currentTarget.checked?messages.map(m=>m.id):[]}/>{t("Select this page")}</label>
     <span class="selection-count" role="status">{checkedRows.length?t('{count} selected',{count:checkedRows.length}):''}</span>
     {#if checkedRows.length}
      <button class="clear-selection" onclick={clearSelection}>{t("Clear selection")}</button>
      <details class="bulk-menu" bind:this={bulkMenu}>
       <summary>{t("Selected actions")}</summary>
       <div class="bulk-actions" aria-label={t("Bulk actions")}>
        {#each bulkActions as item}
         <button disabled={pageBusy||queryPending||bulkBusy||!!filterError} onclick={()=>bulkAction(item.action)}>{t(item.label)}</button>
        {/each}
       </div>
      </details>
     {/if}
    </div>
   {/if}
   <div class="rows" use:restoreRows onscroll={e=>listScroll=e.currentTarget.scrollTop}>
    {#each messages as m (m.id)}
     <div class="triage-row">
      <input type="checkbox" aria-label={t('Select {subject}',{subject:m.subject||t('message')})} checked={checkedRows.includes(m.id)}
       disabled={pageBusy||queryPending||bulkBusy||!!filterError} onchange={e=>toggleRow(m.id,e.currentTarget.checked)}/>
      <button class="message" class:selected={!!current&&sameConversation(m,current)} class:unread-row={m.unread} disabled={queryPending||!!filterError} onclick={()=>open(m)}><div class="row-top"><strong>{m.sender||m.address}</strong><time>{date(m.date)}</time></div><div class="row-subject">{#if m.unread}<span class="unread" aria-hidden="true"></span><span class="visually-hidden">{t("Unread.")} </span>{/if}{m.subject}{#if m.conversation_count>1}<small class="thread-count" aria-label={t('{count} messages',{count:m.conversation_count})}>{m.conversation_count}</small>{/if}</div><p>{m.body||t('Open to download message')}</p>{#if (!account&&accounts.length>1)||m.starred}<div class="row-footer">{#if !account&&accounts.length>1}<span>{m.account}</span>{/if}{#if m.starred}<Star size={13}/>{/if}</div>{/if}</button>
     </div>
    {/each}
    {#if !messages.length&&!pageBusy&&!queryPending&&!filterError}
     <div class="empty">
      <Mail size={28}/><h2>{accounts.length?t('No conversations'):t('Connect an account')}</h2>
      <p>{search||filterSummary.length?t('No cached conversations match this search.'):accounts.length?t('No downloaded mail in this view. Sync to check your provider.'):t('Connect an account to get started.')}</p>
      {#if !search&&!filterSummary.length}<button class="reply" onclick={()=>accounts.length?refresh():settings=true}>{accounts.length?t('Synchronize'):t('Add an account')}</button>{/if}
     </div>
    {/if}
   </div>
   <div class="pagination">
    <button disabled={pageBusy||queryPending||bulkBusy||!!filterError||!cursorStack.length} onclick={previousPage}>{t("Previous")}</button>
    <span>{t("Page")} {cursorStack.length+1} · {messages.length} {t("shown")}</span>
    <button disabled={pageBusy||queryPending||bulkBusy||!!filterError||!nextCursor} onclick={nextPage}>{t("Next")}</button>
   </div>
   <div class="list-bottom">{t("Encrypted cache · bodies download on opening")}</div>
  </section>
 {/if}
 <section class="reader">{#if current}<div class="reader-toolbar" aria-label={t("Actions for selected message")}>
  <div>
   <button title={t("Archive")} disabled={messageActionBusy} onclick={()=>action('archive')}><Archive size={18}/></button>
   <button title={t("Snooze until tomorrow")} onclick={snooze}><Clock size={18}/></button>
   <button title={t("Trash")} disabled={messageActionBusy} onclick={()=>action('trash')}><Trash2 size={18}/></button>
   <button title={t("Toggle star")} disabled={messageActionBusy} onclick={()=>action(current?.starred?'unstar':'star')}><Star size={18} fill={current.starred?'currentColor':'none'}/></button>
   <details class="reader-menu" bind:this={readerMenu}>
    <summary>{t("More actions")}</summary>
    <div class="reader-menu-items">
     <button disabled={!current.loaded||current.unread||messageActionBusy} onclick={()=>action('unread')}>{t("Mark unread")}</button>
     {#if current.folder!=='inbox'}<button disabled={messageActionBusy} onclick={()=>action('inbox')}>{t("Move to Inbox")}</button>{/if}
     {#if current.folder!=='spam'}<button disabled={messageActionBusy} onclick={()=>action('spam')}>{t("Report spam")}</button>{/if}
     {#if readerMailboxes.length}
      <details class="organize-menu">
       <summary>{current.provider==='google'?t('Labels'):t('Move to folder')}</summary>
       {#each readerMailboxes as box (box.id)}
        {#if box.label}
         <label><input type="checkbox" checked={current.labels?.includes(box.id)||false}
          disabled={!current.loaded||messageActionBusy}
          onchange={event=>{
           const enabled=event.currentTarget.checked;
           event.currentTarget.checked=!enabled;
           organize(box,enabled);
          }}/>{box.name}</label>
        {:else}
         <button disabled={!current.loaded||messageActionBusy||current.labels?.includes(box.id)}
          onclick={()=>organize(box,null)}>{box.name}</button>
        {/if}
       {/each}
      </details>
     {/if}
    </div>
   </details>
  </div>
  <button class="focus-button" onclick={()=>focus=!focus}>{focus?t('Exit focus'):t('Focus mode')}</button>
 </div><article><div class="reader-eyebrow">{current.account} / {folderName(current.folder)}</div><h2>{conversation.at(-1)?.subject||current.subject||t('(No subject)')}</h2><p class="conversation-meta">{t(conversation.length===1?'{count} cached message · Oldest first':'{count} cached messages · Oldest first',{count:conversation.length})}{#if current.conversation_key?.startsWith('subject:')} {t("· Grouped by subject")}{/if}</p>{#each conversation as member (member.id)}{#if member.id!==current.id}<button class="conversation-message" onclick={()=>expand(member)} aria-expanded="false"><span><strong>{member.sender||member.address||t('Unknown sender')}</strong>{#if member.unread}<span class="unread"></span>{/if}<small>{member.body||member.subject}</small></span><time datetime={member.date} title={messageDate(member.date)}>{messageDate(member.date)}</time></button>{:else}<section class="conversation-expanded" aria-label={t("Selected message")}><div class="sender"><div class="sender-avatar">{current.sender.slice(0,2).toUpperCase()}</div><div><strong>{current.sender}</strong><span>&lt;{current.address}&gt;</span></div><time datetime={current.date}>{messageDate(current.date)}<small>{folderName(current.folder)} {t("· Selected message")}</small></time></div>{#if current.to?.length||current.cc?.length||current.reply_to?.length}<details class="recipient-details"><summary>{t("Recipients")}</summary>{#if current.to?.length}<p>{t("To:")} {current.to.join(', ')}</p>{/if}{#if current.cc?.length}<p>{t("Cc:")} {current.cc.join(', ')}</p>{/if}{#if current.reply_to?.length}<p>{t("Reply to:")} {current.reply_to.join(', ')}</p>{/if}</details>{/if}{#if current.loaded&&current.html_body}{#key current.id}<MailBody id={current.id} html={current.html_body} text={current.body}/>{/key}{:else}<div class="body">{current.loaded?current.body:loadError||t('Downloading message…')}{#if loadError}<button class="ai-button" onclick={()=>current&&expand(current)}>{t("Try again")}</button>{/if}</div>{/if}{#if current.attachments.length}<div class="attachment-list">{#each current.attachments as f}<button onclick={()=>requestAttachment(f)}><Paperclip size={14}/>{f.name}<small>{Math.ceil(f.size/1024)} {t("KB · Save")}</small></button>{/each}</div>{/if}{#if ai}<div class="ai-result"><span><Sparkles size={15}/>{t("AI suggestion · verify with the source")}</span><p>{ai}</p></div>{/if}<div class="reply-actions"><button class="reply" disabled={!current.loaded||composeBusy} onclick={()=>newDraft('reply')}><ArrowLeft size={16}/>{t("Reply")}</button><button class="ai-button" disabled={!current.loaded||composeBusy} onclick={()=>newDraft('reply_all')}>{t("Reply all")}</button><button class="ai-button" disabled={!current.loaded||composeBusy} onclick={()=>newDraft('forward')}>{t("Forward")}</button><details class="reader-ai"><summary>{t("AI tools")}</summary><div><button class="ai-button" disabled={!current.loaded||aiBusy} onclick={()=>ask('summary')}><Sparkles size={15}/>{t("Summarize")}</button><button class="ai-button" disabled={!current.loaded||aiBusy} onclick={()=>ask('actions')}>{t("Actions")}</button><button class="ai-button" disabled={!current.loaded||aiBusy} onclick={()=>ask('reply')}>{t("Suggest reply")}</button>{#if aiBusy}<button onclick={()=>invoke('cancel_ai')}>{t("Cancel AI")}</button>{/if}</div></details></div></section>{/if}{/each}{#if current.headers_shortened}<p class="privacy-note">{t("Some headers were too long and have been shortened.")}</p>{/if}</article><footer><span><kbd>J</kbd><kbd>K</kbd>{t("navigate")}</span><span><kbd>E</kbd>{t("archive")}</span><span><kbd>R</kbd>{t("reply")}</span></footer>{:else}<div class="empty reader-empty"><Mail size={36}/><h2>{t("Select a conversation")}</h2>{#if status}<p>{status}</p>{/if}</div>{/if}</section></div></main>
 {#snippet jobActions(job:Job)}
  {#if job.status==='pending'}<button onclick={()=>manageJob(job,'cancel_job')}>{t("Cancel")}</button>{/if}
  {#if job.status==='uncertain'&&job.kind==='send'}<button onclick={()=>manageJob(job,'resolve_send')}>{t("I checked Sent · dismiss")}</button>{/if}
  {#if job.status==='failed'&&job.kind==='mutation'}<button onclick={()=>manageJob(job,'retry_mutation')}>{t("Retry")}</button>{/if}
 {/snippet}
 {#if jobs.some(activeStatus)}
  <div class="outbox-strip" aria-label={t("Pending operations")}>
   {#each jobs.filter(activeStatus).slice(0,4) as job (job.id)}
    <div><strong>{job.kind==='send'?t('Outbox'):t('Mail action')} · {t(job.status)}</strong><small>{t(job.detail)}</small>{@render jobActions(job)}</div>
   {/each}
   <div><button onclick={openOperations}>{t("View mail operations")}</button></div>
  </div>
 {/if}
 {#if pendingAttachment}
  <SecurityConfirmation title={t("Save this attachment?")} detail={pendingAttachment.file.name}
   action={t("Save attachment")} oncancel={()=>pendingAttachment=null} onconfirm={confirmAttachment}/>
 {/if}
 {#if notice}<div class="toast" role="status">{notice}<button aria-label={t("Dismiss")} onclick={()=>notice=''}><X size={15}/></button></div>{/if}{#if error}<div class="toast error" role="alert">{error}<button aria-label={t("Dismiss error")} onclick={()=>error=''}><X size={15}/></button></div>{/if}
 {#if settings||compose||palette||consent||operations}<div class="overlay" role="presentation"><div class="modal" class:compose-dialog={compose} use:modal role="dialog" aria-modal="true" aria-label={operations?t('Mail operations'):consent?t('Hosted AI consent'):compose?t('Compose'):settings?t('Settings'):t('Commands')} tabindex="-1"><div class="modal-header"><h2>{operations?t('Mail operations'):consent?t('Share this message with your model?'):compose?(composeMode==='forward'?t('Forward message'):composeMode==='reply_all'?t('Reply all'):composeMode==='reply'?t('Reply'):t('A new conversation')):settings?t('Your preferences'):t('Commands')}</h2><button aria-label={t("Close")} disabled={draftFinishing} onclick={closeModal}><X size={20}/></button></div>
 {#if operations}
  <div class="operations-list" aria-busy={operationsBusy}>
   {#each activePage.jobs as job (job.id)}
    <div class="operation-row">
     <strong>{job.subject||(job.kind==='send'?t('Outbox'):t('Mail action'))}</strong>
     <small>{accounts.find(a=>a.id===job.account_id)?.email||job.account_id} · {t(job.status)}</small>
     <p>{t(job.detail)}</p>
     {@render jobActions(job)}
    </div>
   {:else}
    <p>{operationsBusy?t('Loading operations…'):t('No active mail operations.')}</p>
   {/each}
  </div>
  <div class="pagination">
   <button disabled={operationsBusy||operationOffset===0} onclick={()=>loadOperations(Math.max(0,operationOffset-20))}>{t("Previous")}</button>
   <span>{t("Page")} {Math.floor(operationOffset/20)+1}</span>
   <button disabled={operationsBusy||activePage.next_offset===null} onclick={()=>activePage.next_offset!==null&&loadOperations(activePage.next_offset)}>{t("Next")}</button>
  </div>
 {:else if consent}<p>{t("Destination:")} <strong>{consent.endpoint}</strong></p><p>{t("Model:")} {consent.model}</p><p>{t("This sends the selected message’s sender, subject and up to 12,000 characters of body text. Attachments are excluded. The model provider’s retention and training terms apply; use a provider permitted for this mail account.")}</p><p>{t("Your choice is remembered only for this account and destination. AI still runs only when requested.")}</p><button class="reply" onclick={approve}>{t("Allow this destination and continue")}</button>
 {:else if compose}
  <div class="compose-scroll">
  <fieldset class="compose-fields" disabled={draftFinishing}>
   <label>{t("From")}<select bind:value={from} onchange={resetSource}>{#each accounts as a}<option value={a.id}>{a.email}</option>{/each}</select></label><div data-recipient="to"><RecipientField label={t("To (comma-separated)")} bind:value={to} recent={recentRecipients}/></div>
   {#if !showCc||!showBcc}
    <div class="recipient-controls">
     {#if !showCc}<button type="button" onclick={()=>revealRecipient('cc')}>{t("Add Cc")}</button>{/if}
     {#if !showBcc}<button type="button" onclick={()=>revealRecipient('bcc')}>{t("Add Bcc")}</button>{/if}
    </div>
   {/if}
   {#if showCc}<div data-recipient="cc"><RecipientField label="Cc" bind:value={cc} recent={recentRecipients}/></div>{/if}
   {#if showBcc}<div data-recipient="bcc"><RecipientField label={t("Bcc")} bind:value={bcc} recent={recentRecipients} hint={t("Hidden from other recipients.")}/></div>{/if}
   <label>{t("Subject")}<input bind:value={subject}/></label><label>{t("Message")}<textarea class="message-editor" rows="6" bind:value={body}></textarea></label>
   {#if showSignature}
    <label>{t("Signature")}<textarea rows="3" maxlength="4096" bind:value={signature}></textarea></label>
    <button type="button" onclick={()=>{signature='';showSignature=false}}>{t("Remove signature")}</button>
   {:else if accountSignatures[from]}
    <button type="button" onclick={()=>{signature=accountSignatures[from];showSignature=true}}>{t("Insert signature")}</button>
   {/if}
   <div class="attachment-list">{#each files as f}<button onclick={()=>files=files.filter(x=>x.id!==f.id)}>{f.name}<X size={12}/></button>{/each}</div>{#if forwardChoices.length}<fieldset class="forward-attachments"><legend>{t("Original attachments")}</legend><p>{t("Choose which attachments to include. None are added automatically.")}</p>{#each forwardChoices as f}<label><input type="checkbox" value={f.id} bind:group={forwardSelected}/>{f.name} · {Math.ceil(f.size/1024)} KB</label>{/each}<button disabled={composeBusy||!forwardSelected.length} onclick={includeForwardFiles}>{t("Include selected attachments")}</button></fieldset>{/if}
  </fieldset>
  </div>
  <div class="compose-footer">
  {#if composeError}<div class="compose-error" role="alert" tabindex="-1">{composeError}</div>{/if}
  {#if confirmDiscard}
   <p>{t("Discard this draft?")}</p>
   <div class="modal-actions">
    <button class="reply" disabled={draftFinishing} onclick={()=>finishDraft('discard')}>{t("Discard draft")}</button>
    <button disabled={draftFinishing} onclick={()=>confirmDiscard=false}>{t("Keep editing")}</button>
   </div>
  {:else}
   <div class="modal-actions">
    <button class="reply" disabled={draftFinishing||composeBusy} onclick={()=>finishDraft('send')}><Send size={15}/>{t("Send")}</button>
    <button disabled={draftFinishing} onclick={attach}><Paperclip size={16}/>{t("Attach")}</button>
    <button disabled={draftFinishing} onclick={()=>finishDraft('save')}>{t("Save draft")}</button>
    <button disabled={draftFinishing} onclick={()=>confirmDiscard=true}>{t("Discard")}</button>
   </div>
   <p class="send-delay">{t("Sending starts in 10 seconds. You can cancel during that time.")}</p>
  {/if}
  </div>
 {:else if settings}<LanguagePicker/>{#each accounts as a}<label>{t('Your other addresses for {email}',{email:a.email})}<input aria-label={t('Aliases for {email}',{email:a.email})} bind:value={aliasValues[a.id]} placeholder="alias@example.com, other@example.com"/><small>{t("Excluded from Reply all. This does not enable sending as an alias.")}</small></label><button onclick={()=>saveAliases(a.id)}>{t("Save addresses")}</button>
 <label>{t('Signature for {email}',{email:a.email})}<textarea aria-label={t('Signature for {email}',{email:a.email})} rows="3" maxlength="4096" bind:value={signatureValues[a.id]}></textarea></label>
 <button disabled={signatureSaving} onclick={()=>saveSignature(a.id)}>{t("Save signature")}</button>
 {/each}<label>{t("Reading size:")} {fontSize}px<input type="range" min="14" max="22" bind:value={fontSize}/></label><label>{t("Reading width")}<input type="range" min="580" max="1000" step="20" bind:value={lineWidth}/></label><button class="setting-row" onclick={()=>compact=!compact}>{t("Compact list")}<span>{compact?t('On'):t('Off')}</span></button>{#if current}<button class="setting-row" onclick={revokeAi}>{t("Revoke hosted-AI consent for selected account")}</button>{/if}{#if notificationPreferences}
 <fieldset class="notification-settings">
  <legend>{t("Windows mail notifications")}</legend>
  <label><input type="checkbox" checked={notificationPreferences.enabled} disabled={notificationSaving}
   onchange={e=>{
    const enabled=e.currentTarget.checked;
    e.currentTarget.checked=!enabled;
    saveNotificationPreferences({enabled,previews:notificationPreferences?.previews||false});
   }}/>{t("Notify me about new unread mail")}</label>
  <label><input type="checkbox" checked={notificationPreferences.previews} disabled={notificationSaving||!notificationPreferences.enabled}
   onchange={e=>{
    const previews=e.currentTarget.checked;
    e.currentTarget.checked=!previews;
    saveNotificationPreferences({enabled:true,previews});
   }}/>{t("Show sender and subject in notifications")}</label>
  <p>{t("Notifications appear while Rusty Mail is running. Click a banner to open its message.")}</p>
 </fieldset>
 {/if}<p>{t("Account authorization and mail credentials stay in the OS credential store. Mail and cached files are encrypted locally.")}</p><div class="update-controls"><button class="setting-row" disabled={updateBusy} onclick={async()=>{updateBusy=true;try{updateOffer=await invoke("check_updates");notice=updateOffer?.enabled?(updateOffer.version?t("Signed update available"):t("No update available")):t("Development build: signed update channel is not configured")}catch(e){error=String(e)}finally{updateBusy=false}}}>{t("Check for signed updates")}</button>{#if updateOffer?.version}<p>{t("Version")} {updateOffer.version}{t(". Installation closes Rusty Mail. Save and close the composer first.")}</p><button disabled={updateBusy} onclick={async()=>{updateBusy=true;try{await invoke("install_update",{version:updateOffer?.version})}catch(e){error=String(e)}finally{updateBusy=false}}}>{t("Download, verify and install")}</button>{/if}</div><AccountSetup onchange={()=>refreshAccounts().catch(e=>error=String(e))}/>
 {:else}{#each [t('Compose'),t('Synchronize accounts'),t('Focus mode'),t('Settings'),t('Inbox')] as cmd,i}<button class="command-row" onclick={()=>{palette=false;if(i===0)newDraft();if(i===1)refresh();if(i===2)focus=!focus;if(i===3)settings=true;if(i===4)navigate('inbox')}}>{cmd}<ChevronRight size={16}/></button>{/each}{/if}
 </div></div>{/if}
</div>
<style>
 .app {
  --mail-secondary:#52634a;--mail-border:#b6c1ad;--mail-panel:#fffefa;--mail-focus:#446a36;
 }
 .app.dark {
  --mail-secondary:#b8c6af;--mail-border:#6f7d66;--mail-panel:#22291f;--mail-focus:#a4c58e;
 }
 .app :is(button,summary,input,textarea,select):focus-visible{outline:2px solid var(--mail-focus);outline-offset:2px}
 header{height:52px;padding:0 20px}
 .navigation-toggle{display:none}
 aside{padding:16px 14px 6px;min-height:0}
 .brand{margin:0 6px 16px}
 .compose{flex:none}
 .sidebar-scroll{overflow:auto;min-height:0;flex:1;margin-top:12px;padding:3px 4px}
 .sidebar-scroll nav button{padding:8px 10px;color:var(--mail-secondary)}
 .sidebar-scroll .section-label{margin:13px 8px 5px;color:var(--mail-secondary)}
 .sidebar-bottom{flex:none;border-top:1px solid var(--mail-border);margin-top:5px}
 .sidebar-bottom button{padding:10px}
 .account{gap:7px;padding:7px}
 .account .account-dot{flex:none}
 .account small{color:var(--mail-secondary)}
 .list-heading{padding:15px 18px 10px}
 .list-heading h1{font-size:22px}
 .message-list .search{margin-inline:16px}
 .message-list .list-meta{padding:8px 16px 0;color:var(--mail-secondary)}
 .list-bottom{padding:8px 16px;color:var(--mail-secondary)}
 .mail-filters,.filter-summary,.filter-error{margin-inline:16px}
 .triage-controls{height:40px;flex:none;display:flex;align-items:center;justify-content:space-between;
  gap:5px;margin:0 12px;font-size:11px;white-space:nowrap}
 .selection-count{font-variant-numeric:tabular-nums}
 .clear-selection,.bulk-menu summary{padding:5px 4px;color:var(--mail-secondary)}
 .clear-selection:hover,.bulk-menu summary:hover,.reader-ai summary:hover{text-decoration:underline}
 .bulk-menu,.reader-ai{position:relative}
 .bulk-menu .bulk-actions,.reader-ai>div{position:absolute;right:0;top:calc(100% + 4px);z-index:4;
  display:grid;gap:2px;min-width:max-content;background:var(--mail-panel);color:var(--mail-secondary);
  padding:4px;border:1px solid var(--mail-border);border-radius:6px;box-shadow:0 4px 12px #0002}
 .bulk-actions button,.reader-ai button{justify-content:flex-start;border:0;padding:7px 9px}
 .triage-row .message{padding:8px 14px 8px 8px}
 .triage-row .row-top time{white-space:nowrap;color:var(--mail-secondary)}
 .triage-row .row-top strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .triage-row .row-subject{margin-top:4px}
 .triage-row .message p{margin:3px 0 2px;color:var(--mail-secondary)}
 .triage-row .row-footer span{padding:1px 5px;font-size:9px;color:#52634a}
 .unread-row .row-subject{font-weight:700}
 .unread-row .unread{height:7px;width:7px}
 .visually-hidden{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
 .reader-toolbar,.reader-toolbar .focus-button,.reader-ai,.reply-actions .ai-button{color:var(--mail-secondary)}
 .reader-ai summary{padding:8px;white-space:nowrap;font-size:12px}
 .reader-ai>div{top:auto;bottom:calc(100% + 4px)}
 .reply-actions{gap:8px}
 .modal.compose-dialog{display:flex;flex-direction:column;overflow:hidden;padding:0;width:620px;max-height:calc(100dvh - 32px)}
 .compose-dialog .modal-header{flex:none;margin:0;padding:14px 20px;border-bottom:1px solid var(--mail-border)}
 .compose-scroll{min-height:0;overflow:auto;padding:2px 20px 10px}
 .compose-dialog .compose-fields :global(label:not(.search)){font-size:12px;color:var(--mail-secondary);margin:8px 0}
 .compose-dialog .compose-fields :global(input:not([type=checkbox])),
 .compose-dialog .compose-fields :global(select),
 .compose-dialog .compose-fields :global(textarea){padding:7px 9px;margin-top:4px;border-color:var(--mail-border)}
 .recipient-controls{display:flex;gap:8px}
 .recipient-controls button{font-size:12px;color:var(--mail-secondary);padding:4px 6px}
 .compose-footer{flex:none;padding:10px 20px;border-top:1px solid var(--mail-border)}
 .compose-footer .modal-actions{gap:8px;flex-wrap:wrap}
 .compose-footer .modal-actions button{padding:7px 10px;font-size:12px}
 .compose-footer .send-delay{font-size:11px;margin:3px 0 0;color:var(--mail-secondary)}
 .compose-error{max-height:5em;overflow:auto;margin-bottom:7px;padding:6px;color:inherit;border:1px solid #bd5245;border-radius:5px;font-size:12px}
 @media(max-width:1100px){
  .triage-controls{height:58px;display:grid;grid-template-columns:1fr auto;
   grid-template-rows:repeat(2,minmax(0,1fr));column-gap:8px;row-gap:0}
  .selection-count{justify-self:end}
  .clear-selection{justify-self:start}
  .bulk-menu{justify-self:end}
 }
 @media(max-width:800px){
  .navigation-toggle{display:inline-flex}
  aside.navigation-open{display:flex;position:absolute;inset:52px auto 0 0;z-index:6;width:224px;
   border-right:1px solid var(--mail-border);box-shadow:5px 0 16px #0003}
  .reader-toolbar{padding-inline:11px;gap:4px}
  .reader-toolbar>div{gap:2px}
  .reader article{padding:16px 20px}
  .reader article h2{margin:14px 0 23px}
  .reader .conversation-meta{margin-bottom:14px}
  .reader .conversation-expanded{padding-block:14px}
  .reader .body{padding-block:18px 22px}
 }

 .inbox-unread{margin-left:auto;flex-shrink:0;font-size:11px;font-variant-numeric:tabular-nums}
 .notification-settings{margin:12px 0;border:1px solid #87927755;border-radius:7px}
 .notification-settings legend{font-size:12px}
 .notification-settings label:not(.search){display:flex;align-items:center;gap:8px;color:inherit}
 .notification-settings input[type=checkbox]{display:inline;width:auto;margin:0;padding:0}
 .compose-fields{border:0;padding:0;margin:0;min-width:0}
 .draft-row .row-top strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .mail-filters,.triage-controls,.filter-summary,.filter-error{margin:8px 24px 0;font-size:11px}
 summary{cursor:pointer}
 .filter-fields{display:grid;gap:8px;padding:8px 0}
 .filter-fields label{display:grid;gap:4px;min-width:0}
 .filter-fields .unread-filter,.triage-controls label{display:flex;align-items:center;gap:5px}
 .filter-fields input:not([type=checkbox]){background:inherit;color:inherit;border:1px solid #87927777;border-radius:5px;padding:5px;width:100%;min-width:0}
 .filter-dates{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:8px}
 .filter-summary,.bulk-actions{display:flex;align-items:center;gap:5px;flex-wrap:wrap;overflow-wrap:anywhere}
 .filter-summary span{background:#87927722;border-radius:5px;padding:3px 6px}
 .filter-summary button,.bulk-actions button,.operation-row button{border:1px solid #87927777;padding:4px 7px}
 .filter-error{color:#a45043}
 .triage-controls{margin-top:0;margin-bottom:8px}

 .triage-row{display:grid;grid-template-columns:26px minmax(0,1fr);align-items:center;padding-left:13px}
 .triage-row>input{justify-self:center}
 .triage-row .message{padding-left:10px}

 .mailbox-groups{margin:8px 0}
 .mailbox-groups summary,.organize-menu summary{padding:8px;font-size:11px;overflow-wrap:anywhere}
 .mailbox-groups nav button{padding:7px 8px;text-align:left}
 .mailbox-groups nav :global(svg){flex-shrink:0}
 .mailbox-groups nav span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
 .organize-menu{max-height:230px;max-width:250px;overflow:auto;border-top:1px solid #87927755}
 .organize-menu label{display:flex;align-items:center;gap:6px;padding:5px 8px;overflow-wrap:anywhere}
 .organize-menu button{display:block;max-width:100%;white-space:normal;text-align:left;overflow-wrap:anywhere}
 .reader-menu{position:relative;font-size:11px}
 .reader-menu-items{position:absolute;top:calc(100% + 8px);right:0;z-index:2;display:grid;min-width:126px;background:#fffefa;border:1px solid #87927777;border-radius:6px;padding:4px;color:#596850;box-shadow:0 4px 12px #0002}
 .reader-menu-items button{justify-content:flex-start;white-space:nowrap}
 :global(.dark) .reader-menu-items{background:#22291f;color:#c5cfba}
 .operation-row{border-bottom:1px solid #87927755;padding:10px 0;font-size:12px;overflow-wrap:anywhere}
 .operation-row small{display:block;margin-top:4px;opacity:.8}
 .operation-row p{margin:4px 0}
 .pagination{display:flex;align-items:center;justify-content:space-between;padding:10px;font-size:11px}.pagination button{padding:6px}.attachment-list{display:flex;flex-wrap:wrap;gap:8px;margin-bottom:20px}.attachment-list button{border:1px solid #8a957e55;padding:10px;font-size:12px;flex-wrap:wrap}.attachment-list small{opacity:.7}.outbox-strip{position:fixed;right:20px;bottom:60px;max-width:380px;background:#34472d;color:#fff;border-radius:10px;z-index:5;box-shadow:0 5px 25px #0003;font-size:12px}.outbox-strip>div{padding:12px;border-bottom:1px solid #ffffff22}.outbox-strip small{display:block;margin:6px 0;line-height:1.5}.outbox-strip button{padding:5px 10px;border:1px solid #ffffff55}.modal select{width:100%;padding:10px;margin-top:6px;border:1px solid #87927755;border-radius:5px;background:inherit;color:inherit}.account>div{overflow-wrap:anywhere}.reader-toolbar{gap:10px}.reply-actions{flex-wrap:wrap}
</style>



