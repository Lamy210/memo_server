<script lang="ts">
  import { onMount } from 'svelte';
  import type { PageData } from './$types';

  import MemoEditor from '@/components/features/memo/MemoEditor.svelte';
  import { fetchMemoById, getApiErrorMessage, isUnauthorizedApiError } from '@/lib/api/memo';
  import type { Memo } from '@/lib/api/types';

  export let data: PageData;

  let memo: Memo | undefined;
  let loading = true;
  let errorMessage = '';
  let authRequired = false;

  onMount(async () => {
    try {
      memo = await fetchMemoById(data.id);
    } catch (error) {
      authRequired = isUnauthorizedApiError(error);
      errorMessage = getApiErrorMessage(error, 'メモの取得に失敗しました');
    } finally {
      loading = false;
    }
  });
</script>

{#if loading}
  <div class="grid gap-6 xl:grid-cols-[minmax(0,1fr)_minmax(320px,0.8fr)]">
    <div class="h-[720px] animate-pulse rounded-3xl border border-slate-200 bg-white"></div>
    <div class="h-[440px] animate-pulse rounded-3xl border border-slate-200 bg-white"></div>
  </div>
{:else if errorMessage || !memo}
  <div class="rounded-3xl border border-rose-200 bg-white p-8 shadow-sm">
    <p class="text-sm font-semibold {authRequired ? 'text-amber-700' : 'text-rose-700'}">
      {authRequired ? '認証が必要です' : 'メモを開けませんでした'}
    </p>
    <p class="mt-2 text-sm text-slate-600">{errorMessage || 'メモが見つかりません'}</p>
    <a href="/memos" class="mt-5 inline-flex text-sm font-semibold text-sky-700 hover:text-sky-800">
      メモ一覧へ戻る
    </a>
  </div>
{:else}
  <MemoEditor mode="edit" {memo} />
{/if}
