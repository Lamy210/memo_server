// src/routes/memos/+page.svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { memoStore } from '@/lib/stores';
  import MemoCard from '@/components/features/memo/MemoCard.svelte';
  import { Button } from '@/components/ui';

  let isLoading = $state(true);
  let error = $state<string | null>(null);

  // メモの取得
  onMount(async () => {
    try {
      await memoStore.fetchAll();
    } catch (e) {
      error = 'メモの読み込みに失敗しました';
      console.error('Error fetching memos:', e);
    } finally {
      isLoading = false;
    }
  });
</script>

<div class="space-y-6">
  <div class="flex justify-between items-center">
    <h1 class="text-2xl font-bold text-gray-900">メモ一覧</h1>
    <Button href="/memos/new">新規メモ</Button>
  </div>

  {#if isLoading}
    <div class="flex justify-center py-8">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
    </div>
  {:else if error}
    <div class="bg-red-50 text-red-600 p-4 rounded-lg">
      {error}
    </div>
  {:else if $memoStore.items.length === 0}
    <div class="text-center py-12">
      <p class="text-gray-500 mb-4">メモがありません</p>
      <Button href="/memos/new">最初のメモを作成</Button>
    </div>
  {:else}
    <div class="grid gap-4 grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
      {#each $memoStore.items as memo (memo.id)}
        <a href="/memos/{memo.id}/edit">
          <MemoCard {memo} />
        </a>
      {/each}
    </div>
  {/if}
</div>