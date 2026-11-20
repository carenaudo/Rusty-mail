import {spanish} from './spanish';

type Language='en'|'es';
const preferenceKey='rusty-mail-language';

function initialLanguage():Language {
 try{return localStorage.getItem(preferenceKey)==='es'?'es':'en'}
 catch{return 'en'}
}

export const locale=$state({language:initialLanguage()});
document.documentElement.lang=locale.language;

export function setLanguage(language:Language){
 locale.language=language;
 document.documentElement.lang=language;
 try{localStorage.setItem(preferenceKey,language)}catch{}
}

export function t(text:string,values?:Record<string,string|number>):string{
 const message=locale.language==='es'&&Object.hasOwn(spanish,text)?spanish[text]:text;
 return values?message.replace(/\{(\w+)\}/g,(token,key)=>String(values[key]??token)):message;
}

export function dateLocale():string{
 return locale.language==='es'?'es-AR':'en';
}

const folders:Readonly<Record<string,string>>={
 inbox:'Inbox',starred:'Starred',snoozed:'Snoozed',drafts:'Drafts',
 sent:'Sent',archive:'Archive',spam:'Spam',trash:'Trash',
};

export function folderName(folder:string):string {
 return locale.language==='es'&&Object.hasOwn(folders,folder)?t(folders[folder]):folder;
}
