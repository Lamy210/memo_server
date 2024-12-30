<!-- /app/src/components/features/home/RecentMemos.svelte -->
<script lang="ts">
  // ★ポイント1：Memo型のインポート先を、実際のプロジェクト構造に合わせて修正してください
  import type { Memo } from '@/lib/api/types';
  import { onMount } from 'svelte';
  import { memoStore } from '@/lib/stores';
  import MemoCard from '../memo/MemoCard.svelte';
  import { Button } from '@/components/ui';

  let isLoading = true;
  let error: string | null = null;

  onMount(async () => {
    try {
      await memoStore.fetchAll();
    } catch (e) {
      error = '最近のメモの読み込みに失敗しました';
    } finally {
      isLoading = false;
    }
  });

  /**
   * Svelte 3/4 互換の derived ストアを使う方法
   * -------------------------------------------------
   * memoStore の items から派生して recentMemos を生成します。
   * テンプレート内では $recentMemos としてアクセスできます。
   */
  import { derived } from 'svelte/store';
  export const recentMemos = derived(memoStore, ($memoStore) => {
    if (!$memoStore.items) return [];
    return [...$memoStore.items]
      .sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
      .slice(0, 10);
  });
</script>

<section class="space-y-4">
  <div class="flex justify-between items-center">
    <h2 class="text-2xl font-bold text-gray-900">最近のメモ</h2>
    <Button href="/memos" variant="outline">
      すべて表示
    </Button>
  </div>

  {#if $memoStore.isLoading}
    <div class="flex justify-center py-8">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
    </div>
  {:else if $memoStore.error}
    <div class="bg-red-50 text-red-600 p-4 rounded-lg">
      {$memoStore.error}
    </div>
  {:else if $recentMemos.length === 0}
    <div class="text-center py-8 text-gray-500">
      メモがありません
    </div>
  {:else}
    <div class="grid gap-4 grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
      {#each $recentMemos as memo (memo.id)}
        <a href="/memos/{memo.id}">
          <MemoCard {memo} />
        </a>
      {/each}
    </div>
  {/if}
</section>
