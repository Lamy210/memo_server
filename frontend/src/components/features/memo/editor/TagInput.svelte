<!-- src/components/features/memo/editor/TagInput.svelte -->
<script lang="ts">
    import { onMount } from 'svelte';
    import { editorStore } from '@/lib/stores/editorStore';
    
    let inputElement: HTMLInputElement;
    let inputValue = '';
    let suggestions: string[] = [];
    let isComposing = false;
    let focusedSuggestionIndex = -1;
  
    // 既存のタグからサジェストを生成する関数
    async function updateSuggestions(query: string) {
      if (!query.trim()) {
        suggestions = [];
        return;
      }
  
      try {
        const response = await fetch(`/api/v1/tags/suggest?q=${encodeURIComponent(query)}`);
        if (response.ok) {
          const data = await response.json();
          suggestions = data.filter((tag: string) => 
            !$editorStore.tags.includes(tag) && 
            tag.toLowerCase().includes(query.toLowerCase())
          ).slice(0, 5);
        }
      } catch (error) {
        console.error('Failed to fetch tag suggestions:', error);
        suggestions = [];
      }
    }
  
    // タグの追加処理
    function addTag(tag: string) {
      const normalizedTag = tag.trim().toLowerCase();
      if (
        normalizedTag && 
        !$editorStore.tags.includes(normalizedTag) && 
        $editorStore.tags.length < 10  // 最大10個までに制限
      ) {
        editorStore.updateTags([...$editorStore.tags, normalizedTag]);
      }
      inputValue = '';
      suggestions = [];
      focusedSuggestionIndex = -1;
      inputElement.focus();
    }
  
    // タグの削除処理
    function removeTag(index: number) {
      const newTags = [...$editorStore.tags];
      newTags.splice(index, 1);
      editorStore.updateTags(newTags);
    }
  
    // キーボードイベントの処理
    function handleKeydown(event: KeyboardEvent) {
      if (isComposing) return;
  
      switch (event.key) {
        case 'Enter':
          event.preventDefault();
          if (focusedSuggestionIndex >= 0 && suggestions[focusedSuggestionIndex]) {
            addTag(suggestions[focusedSuggestionIndex]);
          } else {
            addTag(inputValue);
          }
          break;
  
        case 'ArrowDown':
          event.preventDefault();
          if (suggestions.length > 0) {
            focusedSuggestionIndex = (focusedSuggestionIndex + 1) % suggestions.length;
          }
          break;
  
        case 'ArrowUp':
          event.preventDefault();
          if (suggestions.length > 0) {
            focusedSuggestionIndex = focusedSuggestionIndex <= 0 
              ? suggestions.length - 1 
              : focusedSuggestionIndex - 1;
          }
          break;
  
        case 'Backspace':
          if (!inputValue && $editorStore.tags.length > 0) {
            const newTags = [...$editorStore.tags];
            newTags.pop();
            editorStore.updateTags(newTags);
          }
          break;
      }
    }
  
    $: {
      // 入力値が変更されたらサジェストを更新
      if (!isComposing) {
        updateSuggestions(inputValue);
      }
    }
  </script>
  
  <div class="space-y-2">
    <label class="block text-sm font-medium text-gray-700">
      タグ（最大10個まで）
    </label>
    
    <div class="relative">
      <div class="min-h-[42px] p-1.5 border border-gray-300 rounded-lg flex flex-wrap gap-2 bg-white focus-within:ring-2 focus-within:ring-blue-500 focus-within:border-blue-500">
        {#each $editorStore.tags as tag, i (tag)}
          <span class="inline-flex items-center px-2.5 py-1 rounded-full text-sm font-medium bg-blue-100 text-blue-800">
            {tag}
            <button
              type="button"
              class="ml-1.5 h-4 w-4 rounded-full inline-flex items-center justify-center hover:bg-blue-200"
              on:click={() => removeTag(i)}
            >
              <span class="sr-only">タグを削除</span>
              <svg class="h-3 w-3" fill="currentColor" viewBox="0 0 20 20">
                <path d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z" />
              </svg>
            </button>
          </span>
        {/each}
        
        <input
          bind:this={inputElement}
          type="text"
          bind:value={inputValue}
          on:keydown={handleKeydown}
          on:compositionstart={() => isComposing = true}
          on:compositionend={() => isComposing = false}
          placeholder={$editorStore.tags.length < 10 ? "タグを入力..." : "タグは最大10個までです"}
          disabled={$editorStore.tags.length >= 10}
          class="flex-1 min-w-[120px] border-0 p-0.5 focus:ring-0 text-sm"
        />
      </div>
  
      {#if suggestions.length > 0}
        <div class="absolute z-10 mt-1 w-full bg-white shadow-lg max-h-60 rounded-md py-1 text-base overflow-auto focus:outline-none sm:text-sm">
          {#each suggestions as suggestion, i (suggestion)}
            <button
              type="button"
              class="w-full text-left px-4 py-2 text-sm hover:bg-blue-50 {i === focusedSuggestionIndex ? 'bg-blue-50' : ''}"
              on:click={() => addTag(suggestion)}
            >
              {suggestion}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  </div>