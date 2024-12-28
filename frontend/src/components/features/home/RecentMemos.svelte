<!-- src/components/features/home/RecentMemos.svelte -->
<script lang="ts">
    import { onMount } from 'svelte';
    import MemoCard from '../memo/MemoCard.svelte';
    import { Button, Card, Input } from '@/components/ui';
    import { memos } from '../../lib/stores/memoStore';
    import type { Memo } from '@/lib/api/types';
  
    let isLoading = $state(true);
    let error = $state<string | null>(null);
    
    onMount(async () => {
      try {
        await memos.fetchAll();
      } catch (e) {
        error = '最近のメモの読み込みに失敗しました';
      } finally {
        isLoading = false;
      }
    });
  
    // 最新の10件のメモを取得
    $: recentMemos = $memos
      .sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
      .slice(0, 10);
  </script>
  
  <section class="space-y-4">
    <div class="flex justify-between items-center">
      <h2 class="text-2xl font-bold text-gray-900">最近のメモ</h2>
      <Button href="/memos" variant="outline">
        すべて表示
      </Button>
    </div>
  
    {#if isLoading}
      <div class="flex justify-center py-8">
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
      </div>
    {:else if error}
      <div class="bg-red-50 text-red-600 p-4 rounded-lg">
        {error}
      </div>
    {:else if recentMemos.length === 0}
      <div class="text-center py-8 text-gray-500">
        メモがありません
      </div>
    {:else}
      <div class="grid gap-4 grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
        {#each recentMemos as memo (memo.id)}
          <a href="/memos/{memo.id}">
            <MemoCard {memo} />
          </a>
        {/each}
      </div>
    {/if}
  </section>