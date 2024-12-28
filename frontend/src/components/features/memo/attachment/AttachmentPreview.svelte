<!-- src/components/features/memo/attachment/AttachmentPreview.svelte -->
<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import type { Attachment } from '@/lib/types/attachment';
    import { getFileType, isPreviewable } from '@/lib/utils/mime';
    import { Button } from '@/components/ui';
    import FileIcon from './FileIcon.svelte';
  
    export let attachment: Attachment;
    export let onClose: () => void;
  
    let previewContent = $state<string | null>(null);
    let isLoading = $state(true);
    let error = $state<string | null>(null);
  
    // ファイルコンテンツの読み込み
    async function loadPreviewContent() {
      if (!isPreviewable(attachment)) {
        error = 'このファイルはプレビューに対応していません。';
        return;
      }
  
      try {
        isLoading = true;
        error = null;
  
        const response = await fetch(attachment.url);
        if (!response.ok) throw new Error('ファイルの読み込みに失敗しました。');
  
        const fileType = getFileType(attachment.mime_type);
        
        switch (fileType) {
          case 'image':
            previewContent = attachment.url;
            break;
  
          case 'text':
          case 'code':
            const text = await response.text();
            previewContent = text;
            break;
  
          case 'pdf':
            // PDF表示用のiframeソースを設定
            previewContent = `${attachment.url}#toolbar=0`;
            break;
  
          default:
            error = 'このファイル形式はプレビューに対応していません。';
        }
      } catch (e) {
        error = '正常にプレビューを表示できません。';
        console.error('Preview load error:', e);
      } finally {
        isLoading = false;
      }
    }
  
    onMount(() => {
      loadPreviewContent();
    });
  
    // ESCキーでプレビューを閉じる
    function handleKeydown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        onClose();
      }
    }
  </script>
  
  <svelte:window on:keydown={handleKeydown} />
  
  <div
    class="fixed inset-0 bg-black bg-opacity-50 z-50 flex items-center justify-center"
    on:click={onClose}
  >
    <div
      class="bg-white rounded-lg shadow-xl max-w-4xl w-full max-h-[90vh] overflow-hidden"
      on:click|stopPropagation
    >
      <!-- ヘッダー -->
      <div class="px-4 py-3 border-b border-gray-200 flex items-center justify-between">
        <div class="flex items-center space-x-2">
          <FileIcon mimeType={attachment.mime_type} size="lg" />
          <div>
            <h3 class="text-lg font-medium text-gray-900">
              {attachment.filename}
            </h3>
            <p class="text-sm text-gray-500">
              {formatFileSize(attachment.size)}
            </p>
          </div>
        </div>
        
        <Button
          variant="outline"
          size="sm"
          on:click={onClose}
          class="!p-1"
        >
          <span class="material-icons-outlined">close</span>
        </Button>
      </div>
  
      <!-- プレビューコンテンツ -->
      <div class="relative">
        {#if isLoading}
          <div class="absolute inset-0 flex items-center justify-center bg-white">
            <div class="animate-spin rounded-full h-8 w-8 border-2 border-blue-600 border-t-transparent"></div>
          </div>
        {/if}
  
        {#if error}
          <div class="p-8 text-center">
            <div class="text-red-600 mb-4">{error}</div>
            <Button href={attachment.url} target="_blank" rel="noopener noreferrer">
              ダウンロード
            </Button>
          </div>
        {:else if previewContent}
          {#if getFileType(attachment.mime_type) === 'image'}
            <img
              src={previewContent}
              alt={attachment.filename}
              class="max-w-full max-h-[calc(90vh-4rem)] mx-auto"
            />
          {:else if getFileType(attachment.mime_type) === 'pdf'}
            <iframe
              title={attachment.filename}
              src={previewContent}
              class="w-full h-[calc(90vh-4rem)]"
              sandbox="allow-same-origin allow-scripts"
            ></iframe>
          {:else}
            <pre class="max-h-[calc(90vh-4rem)] overflow-auto p-4 text-sm">
              {previewContent}
            </pre>
          {/if}
        {/if}
      </div>
    </div>
  </div>