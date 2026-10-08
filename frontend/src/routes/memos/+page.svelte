<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  import MemoCard from '@/components/features/memo/MemoCard.svelte';
  import { fetchMemosPage, getApiErrorMessage, isUnauthorizedApiError } from '@/lib/api/memo';
  import type { Memo } from '@/lib/api/types';

  let memos: Memo[] = [];
  let loading = true;
  let loadingMore = false;
  let errorMessage = '';
  let authRequired = false;
  let query = '';
  let nextCursor: string | null = null;

  onMount(async () => {
    try {
      const page = await fetchMemosPage();
      memos = page.items;
      nextCursor = page.next_cursor;
    } catch (error) {
      authRequired = isUnauthorizedApiError(error);
      errorMessage = getApiErrorMessage(error, 'メモ一覧の取得に失敗しました');
    } finally {
      loading = false;
    }
  });

  function appendUniqueMemos(current: Memo[], incoming: Memo[]): Memo[] {
    const seen = new Set(current.map((memo) => memo.id));
    const uniqueIncoming = incoming.filter((memo) => {
      if (seen.has(memo.id)) return false;
      seen.add(memo.id);
      return true;
    });
    return [...current, ...uniqueIncoming];
  }

  async function loadMore(): Promise<void> {
    if (!nextCursor || loadingMore) return;

    loadingMore = true;
    errorMessage = '';
    authRequired = false;
    try {
      const page = await fetchMemosPage({ cursor: nextCursor });
      memos = appendUniqueMemos(memos, page.items);
      nextCursor = page.next_cursor;
    } catch (error) {
      authRequired = isUnauthorizedApiError(error);
      errorMessage = getApiErrorMessage(error, 'メモの続きを取得できませんでした');
    } finally {
      loadingMore = false;
    }
  }

  function submitSearch(): void {
    const trimmed = query.trim();
    void goto(trimmed ? `/memos/search?query=${encodeURIComponent(trimmed)}` : '/memos/search');
  }
</script>

<svelte:head>
  <title>メモ | Schnee Memo</title>
</svelte:head>

<section>
  <div class="flex flex-col gap-5 sm:flex-row sm:items-end sm:justify-between">
    <div>
      <p class="text-xs font-semibold uppercase tracking-[0.18em] text-sky-700">ワークスペース</p>
      <h1 class="mt-2 text-3xl font-bold tracking-tight text-slate-950">メモ</h1>
      <p class="mt-2 text-sm leading-6 text-slate-500">アイデア、設計、TODOをひとつの場所に。</p>
    </div>
    <a
      href="/memos/new"
      class="inline-flex items-center justify-center rounded-xl bg-sky-700 px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition hover:bg-sky-800"
    >+ 新しいメモ</a>
  </div>

  <form
    class="mt-8 flex gap-2 rounded-2xl border border-slate-200 bg-white p-2 shadow-sm"
    onsubmit={(event) => {
      event.preventDefault();
      submitSearch();
    }}
  >
    <input
      bind:value={query}
      aria-label="メモを検索"
      placeholder="タイトル・本文を検索…"
      class="min-w-0 flex-1 rounded-xl border-0 bg-transparent px-3 py-2 text-sm text-slate-950 outline-none placeholder:text-slate-500"
    />
    <button
      type="submit"
      class="rounded-xl bg-slate-900 px-4 py-2 text-sm font-semibold text-white transition hover:bg-slate-700"
    >検索</button>
  </form>

  {#if loading}
    <div class="mt-8 grid gap-4 md:grid-cols-2 xl:grid-cols-3" aria-label="メモを読み込み中">
      {#each Array(6) as _}
        <div class="h-48 animate-pulse rounded-2xl border border-slate-200 bg-white"></div>
      {/each}
    </div>
  {:else if errorMessage && memos.length === 0}
    <div
      class="mt-8 rounded-2xl border px-5 py-4 text-sm {authRequired
        ? 'border-amber-200 bg-amber-50 text-amber-800'
        : 'border-rose-200 bg-rose-50 text-rose-700'}"
      role="alert"
    >
      {#if authRequired}
        <p class="font-semibold">認証が必要です</p>
      {/if}
      <p class={authRequired ? 'mt-1' : ''}>{errorMessage}</p>
    </div>
  {:else if memos.length === 0}
    <div class="mt-8 rounded-3xl border border-dashed border-slate-300 bg-white px-6 py-16 text-center">
      <div class="mx-auto grid h-12 w-12 place-items-center rounded-2xl bg-sky-50 text-xl text-sky-700">✎</div>
      <h2 class="mt-4 text-lg font-semibold text-slate-900">最初のメモを書きましょう</h2>
      <p class="mt-2 text-sm text-slate-500">タイトルと本文だけで始められます。</p>
      <a
        href="/memos/new"
        class="mt-6 inline-flex rounded-xl bg-sky-700 px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-sky-800"
      >新しいメモを作成</a>
    </div>
  {:else}
    <div class="mt-8 grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      {#each memos as memo (memo.id)}
        <MemoCard {memo} />
      {/each}
    </div>

    {#if errorMessage}
      <div
        class="mt-6 rounded-2xl border px-5 py-4 text-sm {authRequired
          ? 'border-amber-200 bg-amber-50 text-amber-800'
          : 'border-rose-200 bg-rose-50 text-rose-700'}"
        role="alert"
      >
        {errorMessage}
      </div>
    {/if}

    {#if nextCursor}
      <div class="mt-8 flex justify-center">
        <button
          type="button"
          class="rounded-xl border border-slate-300 bg-white px-5 py-2.5 text-sm font-semibold text-slate-800 shadow-sm transition hover:border-slate-400 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60"
          disabled={loadingMore}
          aria-busy={loadingMore}
          onclick={() => void loadMore()}
        >
          {loadingMore ? '読み込み中…' : 'さらに読み込む'}
        </button>
      </div>
    {/if}
  {/if}
</section>
