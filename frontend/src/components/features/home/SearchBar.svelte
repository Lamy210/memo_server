<!-- /app/src/components/features/home/SearchBar.svelte -->
<script lang="ts">
  import { Input } from '@/components/ui';
  
  // リアクティブな状態
  let searchQuery = $state('');

  function handleSubmit(event: Event) {
    event.preventDefault();
    if (searchQuery.trim()) {
      window.location.href = `/memos/search?q=${encodeURIComponent(searchQuery)}`;
    }
  }
</script>

<form
  onsubmit={handleSubmit}
  class="max-w-2xl mx-auto mb-8"
>
  <div class="relative">
      <!-- bind:value が使用できるように Input.svelte 側を修正 -->
      <Input
          type="text"
          bind:value={searchQuery}
          placeholder="メモを検索..."
          class="pl-10 pr-4 py-3 text-lg"
      />
      <div class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
          <svg
              class="h-5 w-5 text-gray-400"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
          >
              <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
              />
          </svg>
      </div>
  </div>
</form>
