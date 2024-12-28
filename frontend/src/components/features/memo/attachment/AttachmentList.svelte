<!-- src/components/features/memo/attachment/AttachmentList.svelte -->
<script lang="ts">
    import { onMount } from 'svelte';
    import type { Attachment } from '@/lib/types/attachment';
    import { attachments } from '@/lib/stores/attachmentStore';
    import { formatFileSize } from '@/lib/utils/file';
    import { isPreviewable } from '@/lib/utils/mime';
    import { Button } from '@/components/ui';
    import FileIcon from './FileIcon.svelte';
    import AttachmentPreview from './AttachmentPreview.svelte';
  
    export let memoId: string;
  
    let selectedAttachment = $state<Attachment | null>(null);
    let deletingAttachment = $state<Attachment | null>(null);
  
    onMount(() => {
      attachments.fetchAttachments(memoId);
    });
  
    // 削除確認ダイアログ
    async function handleDelete(attachment: Attachment) {
      if (!confirm('この添付ファイルを削除してもよろしいですか？')) return;
      
      try {
        await attachments.deleteAttachment(attachment.id);
      } catch (error) {
        console.error('Failed to delete attachment:', error);
      }
    }
  </script>
  
  <div class="space-y-4">
    {#if $attachments.isLoading}
      <div class="text-center py-4">
        <div class="animate-spin rounded-full h-8 w-8 border-2 border-blue-600 border-t-transparent mx-auto"></div>
      </div>
    {:else if $attachments.error}
      <div class="text-red-600 text-center py-4">
        {$attachments.error}
      </div>
    {:else if $attachments.items.length === 0}
      <div class="text-gray-500 text-center py-4">
        添付ファイルはありません
      </div>
    {:else}
      <div class="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
        {#each $attachments.items as attachment (attachment.id)}
          <div class="border rounded-lg p-4 hover:border-blue-500 transition-colors">
            <div class="flex items-start space-x-3">
              <FileIcon mimeType={attachment.mime_type} size="lg" />
              <div class="flex-1 min-w-0">
                <h4 class="text-sm font-medium text-gray-900 truncate">
                  {attachment.filename}
                </h4>
                <p class="text-sm text-gray-500">
                  {formatFileSize(attachment.size)}
                </p>
              </div>
              
              <div class="flex space-x-2">
                {#if isPreviewable(attachment)}
                  <Button
                    variant="outline"
                    size="sm"
                    class="!p-1"
                    on:click={() => selectedAttachment = attachment}
                  >
                    <span class="material-icons-outlined">visibility</span>
                  </Button>
                {/if}
                
                <Button
                  href={attachment.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  variant="outline"
                  size="sm"
                  class="!p-1"
                >
                  <span class="material-icons-outlined">download</span>
                </Button>
  
                <Button
                  variant="outline"
                  size="sm"
                  class="!p-1 text-red-600 hover:text-red-700"
                  on:click={() => handleDelete(attachment)}
                >
                  <span class="material-icons-outlined">delete</span>
                </Button>
              </div>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
  
  {#if selectedAttachment}
    <AttachmentPreview
      attachment={selectedAttachment}
      onClose={() => selectedAttachment = null}
    />
  {/if}