<!-- src/components/features/memo/attachment/FileUploader.svelte -->
<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import { attachments, uploadProgress } from '@/lib/stores/attachmentStore';
    import { formatFileSize } from '@/lib/utils/file';
    
    export let memoId: string;
    export let maxFileSize = 10 * 1024 * 1024; // 10MB
    export let acceptedTypes = '*/*';
    
    const dispatch = createEventDispatcher();
    
    let dragOver = false;
    let fileInput: HTMLInputElement;
    let error = $state<string | null>(null);
  
    // ドラッグ&ドロップイベントの処理
    function handleDragOver(event: DragEvent) {
      event.preventDefault();
      dragOver = true;
    }
  
    function handleDragLeave() {
      dragOver = false;
    }
  
    async function handleDrop(event: DragEvent) {
      event.preventDefault();
      dragOver = false;
      
      if (!event.dataTransfer?.files) return;
      
      const files = Array.from(event.dataTransfer.files);
      await handleFiles(files);
    }
  
    // ファイル選択処理
    async function handleFileSelect(event: Event) {
      const target = event.target as HTMLInputElement;
      if (!target.files?.length) return;
      
      const files = Array.from(target.files);
      await handleFiles(files);
      
      // 入力をリセット（同じファイルの再選択を可能に）
      target.value = '';
    }
  
    // ファイル処理の共通ロジック
    async function handleFiles(files: File[]) {
      error = null;
  
      for (const file of files) {
        // ファイルサイズの検証
        if (file.size > maxFileSize) {
          error = `ファイルサイズは${formatFileSize(maxFileSize)}以下にしてください`;
          continue;
        }
  
        try {
          await attachments.uploadFile(file, memoId);
          dispatch('upload', { success: true, file });
        } catch (e) {
          error = 'ファイルのアップロードに失敗しました';
          dispatch('upload', { success: false, file, error: e });
        }
      }
    }
  </script>
  
  <div
    class="relative border-2 border-dashed rounded-lg p-6 text-center
           {dragOver ? 'border-blue-500 bg-blue-50' : 'border-gray-300 hover:border-gray-400'}"
    on:dragover={handleDragOver}
    on:dragleave={handleDragLeave}
    on:drop={handleDrop}
  >
    <input
      bind:this={fileInput}
      type="file"
      {acceptedTypes}
      multiple
      class="hidden"
      on:change={handleFileSelect}
    />
  
    <div class="space-y-2">
      <svg
        class="mx-auto h-12 w-12 text-gray-400"
        stroke="currentColor"
        fill="none"
        viewBox="0 0 48 48"
      >
        <path
          d="M28 8H12a4 4 0 00-4 4v20m0 0v4a4 4 0 004 4h20a4 4 0 004-4V28m-4-4h4"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
      
      <div class="text-sm text-gray-600">
        <button
          type="button"
          class="font-medium text-blue-600 hover:text-blue-500"
          on:click={() => fileInput.click()}
        >
          ファイルを選択
        </button>
        <span>またはドラッグ&ドロップ</span>
      </div>
      
      <p class="text-xs text-gray-500">
        最大{formatFileSize(maxFileSize)}まで
      </p>
    </div>
  
    {#if error}
      <div class="mt-2 text-sm text-red-600">
        {error}
      </div>
    {/if}
  
    {#if $uploadProgress.length > 0}
      <div class="mt-4 space-y-2">
        {#each $uploadProgress as upload}
          <div class="relative">
            <div class="text-sm text-gray-600">
              {#if upload.status === 'uploading'}
                アップロード中... ({upload.progress}%)
              {:else if upload.status === 'completed'}
                アップロード完了
              {:else if upload.status === 'error'}
                {upload.error || 'エラーが発生しました'}
              {/if}
            </div>
            
            {#if upload.status === 'uploading'}
              <div class="mt-1 h-2 w-full bg-gray-200 rounded-full overflow-hidden">
                <div
                  class="h-full bg-blue-600 transition-all duration-200"
                  style="width: {upload.progress}%"
                />
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>