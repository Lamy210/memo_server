<script lang="ts">
  import '../app.css';
</script>

<svelte:head>
  <title>Schnee Memo</title>
  <meta
    name="description"
    content="A focused memo workspace for writing, searching, and refining notes."
  />
</svelte:head>

<div class="min-h-screen bg-slate-50 text-slate-900">
  <header class="sticky top-0 z-40 border-b border-slate-200/80 bg-white/90 backdrop-blur">
    <div class="mx-auto flex h-16 max-w-7xl items-center justify-between gap-4 px-4 sm:px-6 lg:px-8">
      <a href="/memos" class="flex items-center gap-3 font-semibold tracking-tight text-slate-950">
        <span class="grid h-9 w-9 place-items-center rounded-xl bg-sky-600 text-lg text-white shadow-sm">❄</span>
        <span>Schnee Memo</span>
      </a>

      <nav class="flex items-center gap-1 text-sm font-medium text-slate-600" aria-label="Main navigation">
        <a href="/memos" class="rounded-lg px-3 py-2 transition hover:bg-slate-100 hover:text-slate-950">
          メモ
        </a>
        <a
          href="/memos/search"
          class="hidden rounded-lg px-3 py-2 transition hover:bg-slate-100 hover:text-slate-950 sm:block"
        >検索</a>
        <a
          href="/memos/new"
          class="ml-1 rounded-xl bg-sky-600 px-3.5 py-2 text-white shadow-sm transition hover:bg-sky-700"
        >新規作成</a>
      </nav>
    </div>
  </header>

  <main class="mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8 lg:py-10">
    <slot></slot>
  </main>
</div>
