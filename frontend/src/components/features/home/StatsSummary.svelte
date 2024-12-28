<!-- src/components/features/home/StatsSummary.svelte -->
<script lang="ts">
    import { Card } from '@/';
    import { memos } from '@/lib/stores/memoStore';
  
    // メモの統計情報を計算
    $: totalMemos = $memos.length;
    $: totalTags = new Set($memos.flatMap(memo => memo.tags)).size;
    $: lastUpdateDate = $memos.length > 0
      ? new Date(Math.max(...$memos.map(m => new Date(m.updated_at).getTime())))
      : null;
  </script>
  
  <div class="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
    <Card padding="lg" class="bg-gradient-to-br from-blue-500 to-blue-600 text-white">
      <div class="space-y-2">
        <h3 class="text-lg font-medium opacity-90">総メモ数</h3>
        <p class="text-3xl font-bold">{totalMemos}</p>
      </div>
    </Card>
  
    <Card padding="lg" class="bg-gradient-to-br from-purple-500 to-purple-600 text-white">
      <div class="space-y-2">
        <h3 class="text-lg font-medium opacity-90">総タグ数</h3>
        <p class="text-3xl font-bold">{totalTags}</p>
      </div>
    </Card>
  
    <Card padding="lg" class="bg-gradient-to-br from-green-500 to-green-600 text-white">
      <div class="space-y-2">
        <h3 class="text-lg font-medium opacity-90">最終更新</h3>
        <p class="text-xl font-bold">
          {#if lastUpdateDate}
            {lastUpdateDate.toLocaleDateString('ja-JP')}
          {:else}
            -
          {/if}
        </p>
      </div>
    </Card>
  </div>