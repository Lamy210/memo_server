<!-- src/components/features/memo/MemoCard.svelte -->
<script lang="ts">
  import { Card } from '@/components/ui';
  import { formatDate } from '@/lib/utils/';
  import type { Memo } from '@/lib/api/types';

  // Runesモードにおける型安全なプロパティ定義
  const props = $props<{
    memo: Memo;  // 必須プロパティとして定義
  }>();

  // メモの内容プレビューの生成（リアクティブな計算）
  const contentPreview = $derived(
    props.memo.content.length > 200 
      ? `${props.memo.content.slice(0, 200)}...` 
      : props.memo.content
  );

  // 最終更新日時のフォーマット（リアクティブな計算）
  const formattedDate = $derived(
    formatDate(new Date(props.memo.updated_at))
  );

  // WAI-ARIA属性の動的生成
  const ariaLabels = $derived({
    'aria-label': `メモ: ${props.memo.title || '無題のメモ'}`,
    'aria-description': contentPreview
  });
</script>

<Card 
  hover={true} 
  padding="md"
  class="cursor-pointer transition-all duration-200 group"
  {...ariaLabels}
>
  <div class="space-y-2">
    <div class="flex items-start justify-between">
      <h3 class="text-lg font-semibold text-gray-900 group-hover:text-blue-600 line-clamp-1">
        {props.memo.title || '無題のメモ'}
      </h3>
      <time datetime={props.memo.updated_at} class="text-sm text-gray-500">
        {formattedDate}
      </time>
    </div>
    
    <p class="text-gray-600 line-clamp-3">
      {contentPreview}
    </p>
    
    {#if props.memo.tags?.length > 0}
      <div class="flex flex-wrap gap-2 mt-3">
        {#each props.memo.tags as tag}
          <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-100 text-blue-800">
            {tag}
          </span>
        {/each}
      </div>
    {/if}
  </div>
</Card>