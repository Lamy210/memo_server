<!-- src/components/features/memo/editor/ToolBar.svelte -->
<script lang="ts">
    import { editorStore } from '@/lib/stores/editorStore';
    import { toolbarGroups } from '@/lib/constants/editorActions';
    import type { ToolbarAction } from '@/lib/types/editor';
  
    export let textareaElement: HTMLTextAreaElement;
  
    // 選択テキストの取得と更新
    function executeAction(action: ToolbarAction) {
      const start = textareaElement.selectionStart;
      const end = textareaElement.selectionEnd;
      const selectedText = textareaElement.value.substring(start, end) || ' ';
      
      const beforeText = textareaElement.value.substring(0, start);
      const afterText = textareaElement.value.substring(end);
      
      const newText = beforeText + action.execute(selectedText) + afterText;
      
      editorStore.updateContent(newText);
      
      // カーソル位置の復元
      requestAnimationFrame(() => {
        textareaElement.focus();
        const newCursorPosition = start + action.execute(selectedText).length;
        textareaElement.setSelectionRange(newCursorPosition, newCursorPosition);
      });
    }
  
    // ドロップダウンメニューの状態管理
    let activeDropdown = $state<string | null>(null);
    
    function toggleDropdown(groupId: string) {
      activeDropdown = activeDropdown === groupId ? null : groupId;
    }
  
    // クリックイベントのハンドリング
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as HTMLElement;
      if (!target.closest('.toolbar-dropdown')) {
        activeDropdown = null;
      }
    }
  </script>
  
  <svelte:window on:click={handleClickOutside} />
  
  <div class="border-b border-gray-200 bg-white sticky top-0 z-10">
    <div class="flex items-center space-x-2 p-2">
      {#each toolbarGroups as group}
        <div class="relative toolbar-dropdown">
          <button
            type="button"
            class="inline-flex items-center px-3 py-1.5 border border-gray-300 rounded-md text-sm font-medium text-gray-700 bg-white hover:bg-gray-50 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
            on:click={() => toggleDropdown(group.id)}
          >
            {group.label}
            <svg class="ml-2 h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
            </svg>
          </button>
  
          {#if activeDropdown === group.id}
            <div class="absolute left-0 mt-1 w-48 rounded-md shadow-lg bg-white ring-1 ring-black ring-opacity-5 z-50">
              <div class="py-1">
                {#each group.actions as action}
                  <button
                    type="button"
                    class="w-full text-left px-4 py-2 text-sm text-gray-700 hover:bg-gray-100 hover:text-gray-900 flex items-center justify-between"
                    on:click={() => {
                      executeAction(action);
                      activeDropdown = null;
                    }}
                  >
                    <span class="flex items-center">
                      <span class="material-icons-outlined text-lg mr-2">
                        {action.icon}
                      </span>
                      {action.label}
                    </span>
                    {#if action.shortcut}
                      <span class="text-xs text-gray-500">{action.shortcut}</span>
                    {/if}
                  </button>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </div>