<!-- src/routes/memos/search/+page.svelte -->
<script lang="ts">
    import { page } from '$app/stores';
    import MemoCard from '@/components/features/memo/MemoCard.svelte';
    import SearchBar from '@/components/features/memo/search/SearchBar.svelte';
    import TagFilter from '@/components/features/memo/search/TagFilter.svelte';
    import { Button } from '@/components/ui';
  
    const props = $props<{
      data: {
        memos: Array<any>;
        searchParams: {
          query: string | null;
          tag: string | null;
        };
      };
    }>();
  
    const query = $page.url.searchParams.get('q') || '';
    const selectedTag = $page.url.searchParams.get('tag');
  </script>
  
  <div class="space-y-6">
    <div class="flex justify-between items-center">
      <h1 class="text-2xl font-bold text-gray-900">検索結果</h1>
      <Button href="/memos/new">新規メモ</Button>
    </div>
  
    <div class="grid gap-6 grid-cols-1 lg:grid-cols-4">
      <!-- 左サイドバー：フィルター -->
      <div class="lg:col-span-1">
        <div class="sticky top-24 space-y-6">
          <TagFilter selected={selectedTag} />
        </div>
      </div>
  
      <!-- メインコンテンツ：検索結果 -->
      <div class="lg:col-span-3 space-y-6">
        <SearchBar initial={query} />
  
        {#if props.data.memos.length === 0}
          <div class="text-center py-12 bg-white rounded-lg shadow-sm">
            <p class="text-gray-500 mb-4">
              {query
                ? `"${query}" に一致するメモが見つかりませんでした`
                : 'メモが見つかりませんでした'}
            </p>
            <Button href="/memos/new">新規メモを作成</Button>
          </div>
        {:else}
          <div class="grid gap-4 grid-cols-1 md:grid-cols-2">
            {#each props.data.memos as memo (memo.id)}
              <a href="/memos/{memo.id}/edit">
                <MemoCard {memo} />
              </a>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>