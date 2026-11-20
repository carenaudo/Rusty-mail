<script lang="ts">
 import MailBody from '../src/MailBody.svelte';
 import {hostile} from './html-fixtures';
 let id=$state('one');
</script>
<div class="app" style="height:auto;min-height:100vh;display:block;padding:24px;max-width:1000px;margin:auto">
 <button data-testid="switch-message" onclick={()=>id=id==='one'?'two':'one'}>Switch message</button>
 <h1>HTML reader security fixture</h1><p>Selected message: {id}</p>
 {#key id}<MailBody {id} html={hostile} text="Plain text alternative. &lt;b&gt;Literal text stays escaped.&lt;/b&gt;"/>{/key}
</div>
