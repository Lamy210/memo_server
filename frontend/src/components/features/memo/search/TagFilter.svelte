<!-- src/components/features/memo/search/TagFilter.svelte -->
<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { memoStore } from '@/lib/stores';
  
  const props = $props<{
    selected?: string | null;
  }>();

  // 全てのユニークなタグを取得
  $: tags = [...new Set($memoStore.items.flatMap(memo => memo.tags))].sort();

  // タグによるフィルタリング
  async function handleTagClick(tag: string) {
    const params = new URLSearchParams($page.url.searchParams);
    if (props.selected === tag) {
      params.delete('tag');
    } else {
      params.set('tag', tag);
    }
    await goto(`?${params.toString()}`);
  }
</script>

<div class="bg-white rounded-lg shadow-sm p-4">
  <h2 class="font-medium text-gray-900 mb-4">タグで絞り込み</h2>
  
  <div class="space-y-2">
    {#each tags as tag}
      <button
        class="flex items-center justify-between w-full px-2 py-1 rounded-md text-sm
               {props.selected === tag ? 'bg-blue-50 text-blue-700' : 'text-gray-600 hover:bg-gray-50'}"
        on:click={() => handleTagClick(tag)}
      >
        <span>{tag}</span>
        <span class="text-xs text-gray-500">
          {$memoStore.items.filter(memo => memo.tags.includes(tag)).length}
        </span>
      </button>
    {/each}
  </div>
  
  {#if tags.length === 0}
    <p class="text-sm text-gray-500 text-center py-2">
      タグがありません
    </p>
  {/if}
</div>