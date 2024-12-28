<!-- src/routes/memos/[id]/edit/+page.svelte -->
<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { goto } from '$app/navigation';
    import { page } from '$app/stores';
    import MarkdownEditor from '@/components/features/memo/editor/MarkdownEditor.svelte';
    import ToolBar from '@/components/features/memo/editor/ToolBar.svelte';
    import Preview from '@/components/features/memo/editor/Preview.svelte';
    import TagInput from '@/components/features/memo/editor/TagInput.svelte';
    import { Button, Input } from '@/components/ui';
    import { editorStore, canSave } from '@/lib/stores/editorStore';
  
    // ページデータの型定義
    interface PageData {
      memo?: {
        id: string;
        title: string;
        content: string;
        tags: string[];
      };
    }
  
    export let data: PageData;
    
    let editorTextarea: HTMLTextAreaElement;
    let isPreviewMode = $state(false);
    let isSaving = $state(false);
    let error = $state<string | null>(null);
  
    // 未保存の変更がある場合の警告
    function beforeUnload(event: BeforeUnloadEvent) {
      if ($editorStore.isDirty) {
        event.preventDefault();
        return event.returnValue = '未保存の変更があります。このページを離れますか？';
      }
    }
  
    // 保存処理の実装
    async function handleSave() {
      if (!$canSave || isSaving) return;
  
      isSaving = true;
      error = null;
  
      try {
        const savedMemo = await editorStore.save();
        
        // 新規作成時は編集画面にリダイレクト
        if (!data.memo) {
          goto(`/memos/${savedMemo.id}/edit`);
        }
      } catch (e) {
        error = '保存に失敗しました。しばらく待ってから再度お試しください。';
        console.error('Failed to save memo:', e);
      } finally {
        isSaving = false;
      }
    }
  
    // 自動保存の設定
    let autoSaveInterval: NodeJS.Timeout;
    function setupAutoSave() {
      autoSaveInterval = setInterval(() => {
        if ($canSave && !isSaving) {
          handleSave();
        }
      }, 30000); // 30秒ごとに自動保存
    }
  
    onMount(() => {
      // 既存のメモデータまたは空の状態でエディタを初期化
      editorStore.initialize(data.memo);
      
      // イベントリスナーの設定
      window.addEventListener('beforeunload', beforeUnload);
      
      // 自動保存の開始
      setupAutoSave();
    });
  
    onDestroy(() => {
      // イベントリスナーのクリーンアップ
      window.removeEventListener('beforeunload', beforeUnload);
      
      // 自動保存の停止
      if (autoSaveInterval) {
        clearInterval(autoSaveInterval);
      }
    });
  
    // キーボードショートカットの処理
    function handleKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key === 's') {
        event.preventDefault();
        handleSave();
      }
    }
  </script>
  
  <svelte:window on:keydown={handleKeyDown} />
  
  <div class="max-w-6xl mx-auto px-4 py-6">
    <header class="mb-6 space-y-4">
      <div class="flex items-center justify-between">
        <Input
          type="text"
          placeholder="タイトルを入力..."
          class="text-2xl font-bold border-none focus:ring-0 px-0"
          value={$editorStore.title}
          on:input={(e) => editorStore.updateTitle(e.currentTarget.value)}
        />
        
        <div class="flex items-center space-x-4">
          <Button
            variant="outline"
            on:click={() => isPreviewMode = !isPreviewMode}
          >
            {isPreviewMode ? 'エディタを表示' : 'プレビューを表示'}
          </Button>
          
          <Button
            on:click={handleSave}
            disabled={!$canSave || isSaving}
          >
            {#if isSaving}
              保存中...
            {:else if $editorStore.lastSavedAt}
              更新を保存
            {:else}
              保存
            {/if}
          </Button>
        </div>
      </div>
  
      <TagInput />
  
      {#if error}
        <div class="bg-red-50 text-red-600 p-4 rounded-lg">
          {error}
        </div>
      {/if}
    </header>
  
    <div class="bg-white rounded-lg shadow-sm">
      {#if isPreviewMode}
        <Preview source={editorTextarea} />
      {:else}
        <div>
          <ToolBar {editorTextarea} />
          <MarkdownEditor bind:textarea={editorTextarea} />
        </div>
      {/if}
    </div>
  </div>