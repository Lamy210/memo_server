// src/routes/shared/+page.svelte
<script lang="ts">
  import { Card } from '@/components/ui';

  let isLoading = $state(false);
  let error = $state<string | null>(null);
  let sharedMemos = $state<any[]>([]);
</script>

<div class="space-y-6">
  <h1 class="text-2xl font-bold text-gray-900">共有されたメモ</h1>

  {#if isLoading}
    <div class="flex justify-center py-8">
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
    </div>
  {:else if error}
    <div class="bg-red-50 text-red-600 p-4 rounded-lg">
      {error}
    </div>
  {:else}
    <Card padding="lg">
      <p class="text-center text-gray-500">
        共有機能は現在開発中です
      </p>
    </Card>
  {/if}
</div>