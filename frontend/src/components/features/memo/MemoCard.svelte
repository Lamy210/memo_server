<script lang="ts">
  import type { Memo } from '@/lib/api/types';

  export let memo: Memo;

  const formatDate = (value: string) =>
    new Intl.DateTimeFormat('ja-JP', {
      dateStyle: 'medium',
      timeStyle: 'short'
    }).format(new Date(value));

  $: excerpt = memo.content.trim().replace(/\s+/g, ' ').slice(0, 140);
</script>

<a
  href={`/memos/${memo.id}/edit`}
  class="group block rounded-2xl border border-slate-200 bg-white p-5 shadow-sm transition hover:-translate-y-0.5 hover:border-sky-300 hover:shadow-md"
>
  <div class="flex items-start justify-between gap-4">
    <div class="min-w-0">
      <h2 class="truncate text-base font-semibold text-slate-950 group-hover:text-sky-700">
        {memo.title}
      </h2>
      <p class="mt-1 text-xs text-slate-500">更新 {formatDate(memo.updated_at)}</p>
    </div>
    <span class="rounded-full bg-slate-100 px-2 py-1 text-[11px] font-medium text-slate-600">
      v{memo.version}
    </span>
  </div>

  <p class="mt-4 line-clamp-3 min-h-12 text-sm leading-6 text-slate-600">
    {excerpt || '本文はまだありません'}
  </p>

  {#if memo.tags.length > 0}
    <div class="mt-4 flex flex-wrap gap-2">
      {#each memo.tags as tag}
        <span class="rounded-full bg-sky-50 px-2.5 py-1 text-xs font-medium text-sky-700">
          #{tag}
        </span>
      {/each}
    </div>
  {/if}
</a>
