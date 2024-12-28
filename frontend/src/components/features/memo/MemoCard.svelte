<!-- src/components/features/memo/MemoCard.svelte -->
<script lang="ts">
    import { Card } from '@/components/ui';
    import { formatDate } from '@/lib/utils/date';
    import type { Memo } from '@/lib/api/types';
  
    export let memo: Memo;
    
    // コンテンツのプレビューを生成（最大200文字）
    $: contentPreview = memo.content.length > 200 
      ? `${memo.content.slice(0, 200)}...` 
      : memo.content;
  
    // 最終更新日時のフォーマット
    $: formattedDate = formatDate(new Date(memo.updated_at));
  </script>
  
  <Card 
    hover={true} 
    padding="md"
    class="cursor-pointer transition-all duration-200 group"
  >
    <div class="space-y-2">
      <div class="flex items-start justify-between">
        <h3 class="text-lg font-semibold text-gray-900 group-hover:text-blue-600 line-clamp-1">
          {memo.title || '無題のメモ'}
        </h3>
        <time datetime={memo.updated_at} class="text-sm text-gray-500">
          {formattedDate}
        </time>
      </div>
      
      <p class="text-gray-600 line-clamp-3">
        {contentPreview}
      </p>
      
      {#if memo.tags?.length > 0}
        <div class="flex flex-wrap gap-2 mt-3">
          {#each memo.tags as tag}
            <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
              {tag}
            </span>
          {/each}
        </div>
      {/if}
    </div>
  </Card>