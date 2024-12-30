<!-- /app/src/components/features/home/StatsSummary.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { derived } from 'svelte/store';
  import { memoStore } from '@/lib/stores';
  import { Card } from '@/components/ui';

  let isInitialized = false;

  // ★ポイント2：Svelte 3/4 互換の derived ストアを使用
  // テンプレート内では $totalMemos, $totalTags, $lastUpdateDate としてアクセス可能
  export const totalMemos = derived(memoStore, ($memoStore) => {
    if (!$memoStore.items) return 0;
    return $memoStore.items.length;
  });

  export const totalTags = derived(memoStore, ($memoStore) => {
    if (!$memoStore.items) return 0;
    return new Set($memoStore.items.flatMap((memo) => memo.tags || [])).size;
  });

  export const lastUpdateDate = derived(memoStore, ($memoStore) => {
    if (!$memoStore.items || $memoStore.items.length === 0) return null;
    const validDates = $memoStore.items
      .map((m) => m.updated_at)
      .filter((date) => date && !isNaN(new Date(date).getTime()))
      .map((date) => new Date(date).getTime());
    if (validDates.length === 0) return null;
    return new Date(Math.max(...validDates));
  });

  function formatDate(date: Date | null): string {
    if (!date || isNaN(date.getTime())) return '-';
    try {
      return new Intl.DateTimeFormat('ja-JP', {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      }).format(date);
    } catch (error) {
      console.error('Date formatting error:', error);
      return '-';
    }
  }

  onMount(async () => {
    try {
      await memoStore.fetchAll();
    } catch (e) {
      console.error('Failed to fetch memos:', e);
    } finally {
      isInitialized = true;
    }
  });
</script>

<div class="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
  <!-- 総メモ数カード -->
  <Card padding="lg" class="bg-gradient-to-br from-blue-500 to-blue-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">総メモ数</h3>
      <p class="text-3xl font-bold">{$totalMemos}</p>
    </div>
  </Card>

  <!-- タグ総数カード -->
  <Card padding="lg" class="bg-gradient-to-br from-purple-500 to-purple-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">総タグ数</h3>
      <p class="text-3xl font-bold">{$totalTags}</p>
    </div>
  </Card>

  <!-- 最終更新日時カード -->
  <Card padding="lg" class="bg-gradient-to-br from-green-500 to-green-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">最終更新</h3>
      <p class="text-xl font-bold">
        {#if isInitialized}
          {formatDate($lastUpdateDate)}
        {:else}
          読み込み中...
        {/if}
      </p>
    </div>
  </Card>
</div>
