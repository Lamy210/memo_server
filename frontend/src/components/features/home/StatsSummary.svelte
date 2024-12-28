<!-- src/components/features/home/StatsSummary.svelte -->
<script lang="ts">
  import { Card } from '@/components/ui';
  import { memos } from '@/lib/stores/memoStore';
  
  // メモの総数を算出（リアクティブな値として定義）
  const totalMemos = $derived($memos.length);

  // ユニークなタグの総数を算出
  const totalTags = $derived(
    new Set($memos.flatMap(memo => memo.tags)).size
  );

  // 最終更新日時の算出
  const lastUpdateDate = $derived(() => {
    if ($memos.length === 0) return null;

    return new Date(
      Math.max(...$memos.map(m => new Date(m.updated_at).getTime()))
    );
  });

  // フォーマット用のヘルパー関数
  function formatDate(date: Date | null): string {
    if (!date) return '-';
    return new Intl.DateTimeFormat('ja-JP', {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    }).format(date);
  }
</script>

<div class="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3">
  <!-- 総メモ数カード -->
  <Card padding="lg" class="bg-gradient-to-br from-blue-500 to-blue-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">総メモ数</h3>
      <p class="text-3xl font-bold">{totalMemos}</p>
    </div>
  </Card>

  <!-- タグ総数カード -->
  <Card padding="lg" class="bg-gradient-to-br from-purple-500 to-purple-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">総タグ数</h3>
      <p class="text-3xl font-bold">{totalTags}</p>
    </div>
  </Card>

  <!-- 最終更新日時カード -->
  <Card padding="lg" class="bg-gradient-to-br from-green-500 to-green-600 text-white">
    <div class="space-y-2">
      <h3 class="text-lg font-medium opacity-90">最終更新</h3>
      <p class="text-xl font-bold">
        {formatDate(lastUpdateDate)}
      </p>
    </div>
  </Card>
</div>