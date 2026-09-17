<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import type { PageData } from './$types';

  import MemoCard from '@/components/features/memo/MemoCard.svelte';
  import { searchMemos } from '@/lib/api/memo';
  import type { Memo } from '@/lib/api/types';

  export let data: PageData;

  let query = data.query;
  let tag = data.tag;
  let page = data.page;
  let items: Memo[] = [];
  let total = 0;
  let totalPages = 0;
  let loading = true;
  let errorMessage = '';

  async function runSearch(): Promise<void> {
    loading = true;
    errorMessage = '';
    try {
      const result = await searchMemos({ query, tag, page, limit: 20 });
      items = result.items;
      total = result.total;
      totalPages = result.total_pages;
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : '検索に失敗しました';
    } finally {
      loading = false;
    }
  }

  function navigate(nextPage = 1): void {
    const params = new URLSearchParams();
    if (query.trim()) params.set('query', query.trim());
    if (tag.trim()) params.set('tag', tag.trim());
    if (nextPage > 1) params.set('page', nextPage.toString());
    void goto(`/memos/search${params.size ? `?${params.toString()}` : ''}`);
  }

  onMount(() => {
    void runSearch();
  });
</script>

<section>
  <div>
    <p class="text-xs font-semibold uppercase tracking-[0.18em] text-sky-600">Search</p>
    <h1 class="mt-2 text-3xl font-bold tracking-tight text-slate-950">メモを検索</h1>
    <p class="mt-2 text-sm text-slate-500">本文・タイトルとタグを組み合わせて探せます。</p>
  </div>

  <form
    class="mt-8 grid gap-3 rounded-2xl border border-slate-200 bg-white p-4 shadow-sm md:grid-cols-[minmax(0,1fr)_220px_auto]"
    onsubmit={(event) => {
      event.preventDefault();
      navigate(1);
    }}
  >
    <label class="block">
      <span class="sr-only">キーワード</span>
      <input
        bind:value={query}
        placeholder="キーワード…"
        class="w-full rounded-xl border border-slate-200 bg-slate-50 px-4 py-3 text-sm outline-none transition focus:border-sky-400 focus:bg-white focus:ring-4 focus:ring-sky-100"
      />
    </label>
    <label class="block">
      <span class="sr-only">タグ</span>
      <input
        bind:value={tag}
        placeholder="タグ…"
        class="w-full rounded-xl border border-slate-200 bg-slate-50 px-4 py-3 text-sm outline-none transition focus:border-sky-400 focus:bg-white focus:ring-4 focus:ring-sky-100"
      />
    </label>
    <button type="submit" class="rounded-xl bg-slate-900 px-5 py-3 text-sm font-semibold text-white hover:bg-slate-700">
      検索
    </button>
  </form>

  <div class="mt-6 flex items-center justify-between gap-4 text-sm text-slate-500">
    <span>{loading ? '検索中…' : `${total}件`}</span>
    <a href="/memos" class="font-semibold text-sky-700 hover:text-sky-800">一覧へ戻る</a>
  </div>

  {#if errorMessage}
    <div class="mt-6 rounded-2xl border border-rose-200 bg-rose-50 px-5 py-4 text-sm text-rose-700">
      {errorMessage}
    </div>
  {:else if loading}
    <div class="mt-6 grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {#each Array(6) as _}
        <div class="h-48 animate-pulse rounded-2xl border border-slate-200 bg-white"></div>
      {/each}
    </div>
  {:else if items.length === 0}
    <div class="mt-6 rounded-3xl border border-dashed border-slate-300 bg-white px-6 py-14 text-center text-sm text-slate-500">
      条件に一致するメモはありません。
    </div>
  {:else}
    <div class="mt-6 grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {#each items as memo (memo.id)}
        <MemoCard {memo} />
      {/each}
    </div>
  {/if}

  {#if !loading && totalPages > 1}
    <div class="mt-8 flex items-center justify-center gap-3">
      <button
        type="button"
        disabled={page <= 1}
        onclick={() => navigate(page - 1)}
        class="rounded-xl border border-slate-200 bg-white px-4 py-2 text-sm font-semibold text-slate-700 disabled:opacity-40"
      >前へ</button>
      <span class="text-sm text-slate-500">{page} / {totalPages}</span>
      <button
        type="button"
        disabled={page >= totalPages}
        onclick={() => navigate(page + 1)}
        class="rounded-xl border border-slate-200 bg-white px-4 py-2 text-sm font-semibold text-slate-700 disabled:opacity-40"
      >次へ</button>
    </div>
  {/if}
</section>
