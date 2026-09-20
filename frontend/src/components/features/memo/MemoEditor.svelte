<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import DOMPurify from 'dompurify';
  import { marked } from 'marked';

  import { SaveRevisionTracker } from '@/lib/autosave';
  import {
    ApiError,
    createMemo,
    deleteMemo,
    getApiErrorMessage,
    isUnauthorizedApiError,
    updateMemo
  } from '@/lib/api/memo';
  import type { Memo } from '@/lib/api/types';

  export let mode: 'create' | 'edit';
  export let memo: Memo | undefined = undefined;

  const revisions = new SaveRevisionTracker();

  let title = memo?.title ?? '';
  let content = memo?.content ?? '';
  let tagsText = memo?.tags.join(', ') ?? '';
  let version = memo?.version ?? 1;
  let saving = false;
  let deleting = false;
  let status: 'saved' | 'dirty' | 'saving' | 'conflict' | 'error' =
    mode === 'edit' ? 'saved' : 'dirty';
  let errorMessage = '';
  let authRequired = false;
  let saveTimeout: ReturnType<typeof setTimeout> | undefined;

  $: tags = tagsText
    .split(',')
    .map((tag) => tag.trim())
    .filter(Boolean)
    .slice(0, 10);
  $: canSave = title.trim().length > 0 && content.trim().length > 0 && !saving;
  $: previewHtml = DOMPurify.sanitize(marked.parse(content, { async: false }) as string);

  function scheduleAutosave(): void {
    if (saveTimeout) clearTimeout(saveTimeout);
    if (mode !== 'edit' || authRequired) return;

    saveTimeout = setTimeout(() => {
      saveTimeout = undefined;
      void save();
    }, 1200);
  }

  function markDirty(): void {
    revisions.markDirty();
    if (authRequired) {
      status = 'error';
      return;
    }

    status = 'dirty';
    errorMessage = '';
    scheduleAutosave();
  }

  async function save(): Promise<void> {
    if (!canSave || saving) return;

    if (saveTimeout) {
      clearTimeout(saveTimeout);
      saveTimeout = undefined;
    }

    const revisionBeingSaved = revisions.snapshot();
    saving = true;
    status = 'saving';
    errorMessage = '';

    try {
      if (mode === 'create') {
        const created = await createMemo({
          title: title.trim(),
          content,
          tags
        });
        authRequired = false;
        status = 'saved';
        await goto(`/memos/${created.id}/edit`, { replaceState: true });
        return;
      }

      if (!memo) return;
      const updated = await updateMemo(memo.id, {
        title: title.trim(),
        content,
        tags,
        version
      });
      memo = updated;
      version = updated.version;
      authRequired = false;

      if (revisions.isCurrent(revisionBeingSaved)) {
        status = 'saved';
      } else {
        status = 'dirty';
        scheduleAutosave();
      }
    } catch (error) {
      if (isUnauthorizedApiError(error)) {
        authRequired = true;
        if (saveTimeout) {
          clearTimeout(saveTimeout);
          saveTimeout = undefined;
        }
        status = 'error';
        errorMessage = getApiErrorMessage(error, '保存に失敗しました');
      } else if (error instanceof ApiError && error.status === 409) {
        status = 'conflict';
        errorMessage = '別の更新が先に保存されています。再読み込みして内容を確認してください。';
      } else {
        status = 'error';
        errorMessage = getApiErrorMessage(error, '保存に失敗しました');
      }
    } finally {
      saving = false;
    }
  }

  async function removeMemo(): Promise<void> {
    if (!memo || deleting) return;
    if (!window.confirm(`「${memo.title}」を削除しますか？この操作は取り消せません。`)) return;

    deleting = true;
    errorMessage = '';
    try {
      await deleteMemo(memo.id);
      authRequired = false;
      await goto('/memos');
    } catch (error) {
      if (isUnauthorizedApiError(error)) {
        authRequired = true;
        if (saveTimeout) {
          clearTimeout(saveTimeout);
          saveTimeout = undefined;
        }
      }
      status = 'error';
      errorMessage = getApiErrorMessage(error, '削除に失敗しました');
      deleting = false;
    }
  }

  function handleShortcut(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      void save();
    }
  }

  onMount(() => {
    window.addEventListener('keydown', handleShortcut);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', handleShortcut);
    if (saveTimeout) clearTimeout(saveTimeout);
  });
</script>

<div class="grid gap-6 xl:grid-cols-[minmax(0,1fr)_minmax(320px,0.8fr)]">
  <section class="rounded-3xl border border-slate-200 bg-white shadow-sm">
    <div class="border-b border-slate-100 px-6 py-5 sm:px-8">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p class="text-xs font-semibold uppercase tracking-[0.18em] text-sky-700">
            {mode === 'create' ? 'New note' : 'Editor'}
          </p>
          <h1 class="mt-1 text-2xl font-bold tracking-tight text-slate-950">
            {mode === 'create' ? '新しいメモ' : 'メモを編集'}
          </h1>
        </div>
        <div class="flex items-center gap-2 text-xs font-medium">
          {#if status === 'saving'}
            <span class="rounded-full bg-amber-50 px-3 py-1.5 text-amber-700">保存中…</span>
          {:else if status === 'saved'}
            <span class="rounded-full bg-emerald-50 px-3 py-1.5 text-emerald-700">保存済み</span>
          {:else if status === 'conflict'}
            <span class="rounded-full bg-rose-50 px-3 py-1.5 text-rose-700">競合</span>
          {:else if status === 'error'}
            <span class="rounded-full bg-rose-50 px-3 py-1.5 text-rose-700">エラー</span>
          {:else}
            <span class="rounded-full bg-slate-100 px-3 py-1.5 text-slate-600">未保存</span>
          {/if}
          {#if mode === 'edit'}
            <span class="text-slate-500">v{version}</span>
          {/if}
        </div>
      </div>
    </div>

    <form
      class="space-y-6 p-6 sm:p-8"
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <label class="block">
        <span class="mb-2 block text-sm font-semibold text-slate-700">タイトル</span>
        <input
          bind:value={title}
          oninput={markDirty}
          maxlength="160"
          placeholder="例: リリース前チェックリスト"
          class="w-full rounded-2xl border border-slate-200 bg-slate-50 px-4 py-3 text-base text-slate-950 outline-none transition placeholder:text-slate-500 focus:border-sky-400 focus:bg-white focus:ring-4 focus:ring-sky-100"
        />
      </label>

      <label class="block">
        <span class="mb-2 flex items-center justify-between text-sm font-semibold text-slate-700">
          <span>本文</span>
          <span class="font-normal text-slate-500">Markdown対応</span>
        </span>
        <textarea
          bind:value={content}
          oninput={markDirty}
          rows="18"
          placeholder="考えたこと、TODO、コード断片などを自由に記録…"
          class="min-h-[420px] w-full resize-y rounded-2xl border border-slate-200 bg-slate-50 px-4 py-4 font-mono text-sm leading-7 text-slate-900 outline-none transition placeholder:font-sans placeholder:text-slate-500 focus:border-sky-400 focus:bg-white focus:ring-4 focus:ring-sky-100"
        ></textarea>
      </label>

      <label class="block">
        <span class="mb-2 block text-sm font-semibold text-slate-700">タグ</span>
        <input
          bind:value={tagsText}
          oninput={markDirty}
          placeholder="rust, architecture, todo"
          class="w-full rounded-2xl border border-slate-200 bg-slate-50 px-4 py-3 text-sm text-slate-950 outline-none transition placeholder:text-slate-500 focus:border-sky-400 focus:bg-white focus:ring-4 focus:ring-sky-100"
        />
        <span class="mt-2 block text-xs text-slate-500">カンマ区切り、最大10個</span>
      </label>

      {#if errorMessage}
        <div
          class="rounded-2xl border px-4 py-3 text-sm {authRequired
            ? 'border-amber-200 bg-amber-50 text-amber-800'
            : 'border-rose-200 bg-rose-50 text-rose-700'}"
          role="alert"
        >
          {#if authRequired}
            <p class="font-semibold">認証が必要です。自動保存を停止しました。</p>
          {/if}
          <p class={authRequired ? 'mt-1' : ''}>{errorMessage}</p>
        </div>
      {/if}

      <div class="flex flex-wrap items-center justify-between gap-3 border-t border-slate-100 pt-5">
        <div class="flex gap-2">
          <a
            href="/memos"
            class="rounded-xl border border-slate-200 px-4 py-2.5 text-sm font-semibold text-slate-600 transition hover:bg-slate-50"
          >一覧へ戻る</a>
          {#if mode === 'edit'}
            <button
              type="button"
              onclick={() => void removeMemo()}
              disabled={deleting}
              class="rounded-xl px-4 py-2.5 text-sm font-semibold text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-50"
            >{deleting ? '削除中…' : '削除'}</button>
          {/if}
        </div>
        <button
          type="submit"
          disabled={!canSave}
          class="rounded-xl bg-sky-700 px-5 py-2.5 text-sm font-semibold text-white shadow-sm transition hover:bg-sky-800 disabled:cursor-not-allowed disabled:bg-slate-200 disabled:text-slate-600"
        >{saving ? '保存中…' : '保存'} <span class="ml-1 text-sky-100">⌘S</span></button>
      </div>
    </form>
  </section>

  <aside class="xl:sticky xl:top-24 xl:self-start">
    <div class="rounded-3xl border border-slate-200 bg-white p-6 shadow-sm sm:p-8">
      <div class="mb-5 flex items-center justify-between">
        <div>
          <p class="text-xs font-semibold uppercase tracking-[0.18em] text-slate-500">Preview</p>
          <h2 class="mt-1 font-semibold text-slate-900">Markdownプレビュー</h2>
        </div>
        <span class="rounded-full bg-slate-100 px-2.5 py-1 text-xs text-slate-500">Live</span>
      </div>
      {#if content.trim()}
        <article class="prose prose-slate max-w-none break-words prose-pre:overflow-auto">
          {@html previewHtml}
        </article>
      {:else}
        <div class="rounded-2xl border border-dashed border-slate-200 bg-slate-50 px-5 py-12 text-center text-sm text-slate-500">
          本文を入力するとここにプレビューされます。
        </div>
      {/if}
    </div>
  </aside>
</div>
