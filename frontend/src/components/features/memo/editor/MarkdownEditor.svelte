<!-- src/components/features/memo/editor/MarkdownEditor.svelte -->
<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { editorStore, canSave } from '@/lib/stores/editorStore';
    import { Button } from '@/components/ui';
    
    let textarea: HTMLTextAreaElement;
    let isComposing = false;
  
    // テキストエリアの高さ自動調整
    function adjustTextareaHeight() {
      if (textarea) {
        textarea.style.height = 'auto';
        textarea.style.height = `${textarea.scrollHeight}px`;
      }
    }
  
    // キーボードショートカットの処理
    function handleKeydown(event: KeyboardEvent) {
      if (isComposing) return;
  
      // Cmd/Ctrl + S で保存
      if ((event.metaKey || event.ctrlKey) && event.key === 's') {
        event.preventDefault();
        if ($canSave) {
          editorStore.save();
        }
      }
  
      // タブキーでインデント
      if (event.key === 'Tab') {
        event.preventDefault();
        const start = textarea.selectionStart;
        const end = textarea.selectionEnd;
        
        const value = $editorStore.content;
        const newValue = value.substring(0, start) + '    ' + value.substring(end);
        
        editorStore.updateContent(newValue);
        
        // カーソル位置の調整
        setTimeout(() => {
          textarea.selectionStart = textarea.selectionEnd = start + 4;
        }, 0);
      }
    }
  
    onMount(() => {
      adjustTextareaHeight();
    });
  </script>
  
  <div class="relative">
    <textarea
      bind:this={textarea}
      class="w-full min-h-[300px] px-4 py-3 text-base leading-relaxed
             border border-gray-300 rounded-lg focus:ring-2 focus:ring-blue-500
             focus:border-blue-500 transition-colors resize-none"
      placeholder="マークダウン形式で入力できます..."
      value={$editorStore.content}
      on:input={() => {
        editorStore.updateContent(textarea.value);
        adjustTextareaHeight();
      }}
      on:keydown={handleKeydown}
      on:compositionstart={() => isComposing = true}
      on:compositionend={() => isComposing = false}
    />
  
    <!-- 保存状態インジケータ -->
    <div class="absolute bottom-4 right-4 flex items-center gap-2">
      {#if $editorStore.isSaving}
        <span class="text-gray-500 text-sm">保存中...</span>
      {:else if $editorStore.lastSavedAt}
        <span class="text-gray-500 text-sm">
          最終保存: {$editorStore.lastSavedAt.toLocaleTimeString()}
        </span>
      {/if}
    </div>
  </div>