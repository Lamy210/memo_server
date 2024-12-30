<!-- src/components/features/memo/search/SearchBar.svelte -->
<script lang="ts">
    import { goto } from '$app/navigation';
    import { Input } from '@/components/ui';
    
    const props = $props<{
      initial?: string;
    }>();
  
    let searchQuery = $state(props.initial ?? '');
    let isSearching = $state(false);
  
    async function handleSubmit(event: Event) {
      event.preventDefault();
      if (!searchQuery.trim()) return;
  
      isSearching = true;
      try {
        await goto(`/memos/search?q=${encodeURIComponent(searchQuery.trim())}`);
      } finally {
        isSearching = false;
      }
    }
  </script>
  
  <form
    on:submit={handleSubmit}
    class="relative"
  >
    <Input
      type="text"
      bind:value={searchQuery}
      placeholder="メモを検索..."
      class="pl-10 pr-4 py-3 text-lg"
      disabled={isSearching}
    />
    
    <div class="absolute inset-y-0 left-0 pl-3 flex items-center pointer-events-none">
      {#if isSearching}
        <div class="animate-spin h-5 w-5 border-2 border-blue-600 rounded-full border-t-transparent" />
      {:else}
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
      {/if}
    </div>
  </form>