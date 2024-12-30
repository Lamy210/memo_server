<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import '../app.css';
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  
  let { children } = $props();
  let isSidebarOpen = $state(false);
  let isMobile = $state(false);
  let isUserMenuOpen = $state(false);
  let isNotificationOpen = $state(false);

  // レスポンシブ対応のためのリサイズ監視
  onMount(() => {
    const checkMobile = () => {
      isMobile = window.innerWidth < 768;
      if (!isMobile) isSidebarOpen = true;
    };
    
    checkMobile();
    window.addEventListener('resize', checkMobile);
    
    return () => {
      window.removeEventListener('resize', checkMobile);
    };
  });

  // クリックアウトサイドの処理
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!target.closest('.user-menu')) {
      isUserMenuOpen = false;
    }
    if (!target.closest('.notification-menu')) {
      isNotificationOpen = false;
    }
  }

  // 通知メニューの切り替え
  function toggleNotifications() {
    isNotificationOpen = !isNotificationOpen;
    isUserMenuOpen = false;
  }

  // ユーザーメニューの切り替え
  function toggleUserMenu() {
    isUserMenuOpen = !isUserMenuOpen;
    isNotificationOpen = false;
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div class="min-h-screen bg-gray-50">
  <!-- ヘッダー -->
  <header class="fixed top-0 left-0 right-0 bg-white shadow-sm z-50">
    <div class="px-4 py-3 flex items-center justify-between">
      <div class="flex items-center space-x-4">
        <!-- モバイルメニューボタン -->
        {#if isMobile}
          <button
            class="p-2 rounded-lg hover:bg-gray-100"
            onclick={() => isSidebarOpen = !isSidebarOpen}
            aria-label="メニューを切り替え"
          >
            <svg
              class="w-6 h-6"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              {#if isSidebarOpen}
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M6 18L18 6M6 6l12 12"
                />
              {:else}
                <path
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  stroke-width="2"
                  d="M4 6h16M4 12h16M4 18h16"
                />
              {/if}
            </svg>
          </button>
        {/if}
        <h1 class="text-xl font-semibold">メモアプリ</h1>
      </div>
      
      <div class="flex items-center space-x-4">
        <!-- 通知ボタン -->
        <div class="relative notification-menu">
          <button 
            class="p-2 rounded-lg hover:bg-gray-100"
            onclick={toggleNotifications}
            aria-label="通知"
          >
            <svg
              class="w-6 h-6"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M15 17h5l-1.405-1.405A2.032 2.032 0 0118 14.158V11a6.002 6.002 0 00-4-5.659V5a2 2 0 10-4 0v.341C7.67 6.165 6 8.388 6 11v3.159c0 .538-.214 1.055-.595 1.436L4 17h5m6 0v1a3 3 0 11-6 0v-1m6 0H9"
              />
            </svg>
          </button>

          {#if isNotificationOpen}
            <div class="absolute right-0 mt-2 w-80 bg-white rounded-lg shadow-lg py-1 z-50">
              <div class="px-4 py-2 border-b border-gray-200">
                <h3 class="text-sm font-medium text-gray-900">通知</h3>
              </div>
              <div class="max-h-96 overflow-y-auto">
                <p class="px-4 py-3 text-sm text-gray-500 text-center">
                  新しい通知はありません
                </p>
              </div>
            </div>
          {/if}
        </div>
        
        <!-- ユーザーメニュー -->
        <div class="relative user-menu">
          <button
            class="p-2 rounded-lg hover:bg-gray-100"
            onclick={toggleUserMenu}
            aria-label="ユーザーメニュー"
          >
            <svg
              class="w-6 h-6"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"
              />
            </svg>
          </button>

          {#if isUserMenuOpen}
            <div class="absolute right-0 mt-2 w-48 bg-white rounded-lg shadow-lg py-1 z-50">
              <a
                href="/profile"
                class="block px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
              >
                プロフィール
              </a>
              <a
                href="/settings"
                class="block px-4 py-2 text-sm text-gray-700 hover:bg-gray-100"
              >
                設定
              </a>
              <button
                class="block w-full text-left px-4 py-2 text-sm text-red-600 hover:bg-gray-100"
                onclick={() => {/* ログアウト処理 */}}
              >
                ログアウト
              </button>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </header>

  <!-- メインコンテンツ -->
  <div class="pt-16 flex">
    <!-- サイドバー -->
    {#if isSidebarOpen || !isMobile}
      <aside class="fixed left-0 top-16 bottom-0 w-64 bg-white border-r border-gray-200 overflow-y-auto">
        <nav class="px-4 py-4 space-y-2">
          <a
            href="/"
            class="flex items-center space-x-2 px-4 py-2 rounded-lg {$page.url.pathname === '/' ? 'bg-blue-50 text-blue-700' : 'hover:bg-gray-50'}"
          >
            <svg
              class="w-5 h-5"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6"
              />
            </svg>
            <span>ホーム</span>
          </a>
          
          <a
            href="/memos"
            class="flex items-center space-x-2 px-4 py-2 rounded-lg {$page.url.pathname.startsWith('/memos') ? 'bg-blue-50 text-blue-700' : 'hover:bg-gray-50'}"
          >
            <svg
              class="w-5 h-5"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
              />
            </svg>
            <span>メモ</span>
          </a>
          
          <a
            href="/shared"
            class="flex items-center space-x-2 px-4 py-2 rounded-lg {$page.url.pathname.startsWith('/shared') ? 'bg-blue-50 text-blue-700' : 'hover:bg-gray-50'}"
          >
            <svg
              class="w-5 h-5"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M17 8h2a2 2 0 012 2v6a2 2 0 01-2 2h-2v4l-4-4H9a1.994 1.994 0 01-1.414-.586m0 0L11 14h4a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2v4l.586-.586z"
              />
            </svg>
            <span>共有</span>
          </a>
          
          <a
            href="/settings"
            class="flex items-center space-x-2 px-4 py-2 rounded-lg {$page.url.pathname.startsWith('/settings') ? 'bg-blue-50 text-blue-700' : 'hover:bg-gray-50'}"
          >
            <svg
              class="w-5 h-5"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
              />
              <path
                stroke-linecap="round"
                stroke-linejoin="round"
                stroke-width="2"
                d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
              />
            </svg>
            <span>設定</span>
          </a>
        </nav>
      </aside>
    {/if}

    <!-- メインコンテンツエリア -->
    <main class="{isSidebarOpen && !isMobile ? 'ml-64' : ''} flex-1 p-6">
      {@render children()}
    </main>
  </div>
</div>