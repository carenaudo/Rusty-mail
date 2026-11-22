<script lang="ts">
 import {locale,setLanguage,t} from './locale.svelte';
</script>

<label class="language-picker">{t('Language')}
 <select class="option-select" value={locale.language} onchange={event=>setLanguage(event.currentTarget.value==='es'?'es':'en')}>
  <option value="en" lang="en">English</option>
  <option value="es" lang="es">Español</option>
 </select>
</label>
