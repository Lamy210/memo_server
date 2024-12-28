<!-- src/components/features/memo/editor/Preview.svelte -->
<script lang="ts">
    import { onMount } from 'svelte';
    import { convertMarkdown } from '@/lib/utils/markdown';
    import { editorStore } from '@/lib/stores/editorStore';
  
    let scrollSync = $state(true);
    let previewElement: HTMLDivElement;
    let sourceElement: HTMLTextAreaElement;
  
    export let source: HTMLTextAreaElement;
    sourceElement = source;
  
    // プレビューのスクロール同期
    function syncScroll() {
      if (!scrollSync || !sourceElement || !previewElement) return;
  
      const sourceHeight = sourceElement.scrollHeight - sourceElement.clientHeight;
      const sourceScrolled = sourceElement.scrollTop / sourceHeight;
      
      const previewHeight = previewElement.scrollHeight - previewElement.clientHeight;
      previewElement.scrollTop = sourceScrolled * previewHeight;
    }
  
    // ソース変更時のスクロール同期
    $: if ($editorStore.content) {
      // DOMの更新を待ってからスクロール同期を実行
      requestAnimationFrame(syncScroll);
    }
  
    // コンポーネントのマウント時の処理
    onMount(() => {
      if (sourceElement) {
        sourceElement.addEventListener('scroll', syncScroll);
        return () => sourceElement.removeEventListener('scroll', syncScroll);
      }
    });
  </script>
  
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-medium text-gray-900">プレビュー</h2>
      <label class="inline-flex items-center">
        <input
          type="checkbox"
          bind:checked={scrollSync}
          class="rounded border-gray-300 text-blue-600 shadow-sm focus:border-blue-300 focus:ring focus:ring-blue-200 focus:ring-opacity-50"
        >
        <span class="ml-2 text-sm text-gray-600">スクロール同期</span>
      </label>
    </div>
  
    <div
      bind:this={previewElement}
      class="prose prose-sm max-w-none overflow-y-auto bg-white rounded-lg shadow-sm border border-gray-200 p-4"
      style="height: calc(100vh - 400px);"
    >
      {@html convertMarkdown($editorStore.content)}
    </div>
  </div>
  
  <style>
    /* Markdownスタイルのカスタマイズ */
    :global(.prose pre) {
      background-color: #f8f9fa;
      border-radius: 0.375rem;
      padding: 1rem;
      margin: 1rem 0;
    }
  
    :global(.prose code) {
      color: #1a1a1a;
      background-color: rgba(175, 184, 193, 0.2);
      padding: 0.2em 0.4em;
      border-radius: 0.25rem;
      font-size: 0.875em;
    }
  
    :global(.prose pre code) {
      color: inherit;
      background-color: transparent;
      padding: 0;
    }
  
    :global(.prose blockquote) {
      border-left: 4px solid #e5e7eb;
      padding-left: 1rem;
      color: #4b5563;
    }
  
    :global(.prose table) {
      width: 100%;
      border-collapse: collapse;
    }
  
    :global(.prose th),
    :global(.prose td) {
      padding: 0.5rem;
      border: 1px solid #e5e7eb;
    }
  
    :global(.prose th) {
      background-color: #f9fafb;
    }
  </style>