// src/lib/stores/editorStore.ts
import { writable, derived } from 'svelte/store';
import type { Memo } from '../api/types';
import * as memoApi from '../api/memo';

interface EditorState {
  id?: string;
  title: string;
  content: string;
  tags: string[];
  attachments: File[];
  isSaving: boolean;
  lastSavedAt?: Date;
  isDirty: boolean;
}

function createEditorStore() {
  // 初期状態の定義
  const initialState: EditorState = {
    title: '',
    content: '',
    tags: [],
    attachments: [],
    isSaving: false,
    isDirty: false
  };

  const { subscribe, set, update } = writable<EditorState>(initialState);

  // 自動保存の設定（デバウンス処理）
  let saveTimeout: NodeJS.Timeout;

  // 下書き保存用のローカルストレージキー
  const DRAFT_KEY = 'memo_draft';

  return {
    subscribe,
    
    // エディタの初期化
    initialize: (memo?: Memo) => {
      if (memo) {
        set({
          ...initialState,
          id: memo.id,
          title: memo.title,
          content: memo.content,
          tags: memo.tags,
          lastSavedAt: new Date(memo.updated_at)
        });
      } else {
        // 下書きの復元を試みる
        const draft = localStorage.getItem(DRAFT_KEY);
        if (draft) {
          const parsedDraft = JSON.parse(draft);
          set({
            ...initialState,
            ...parsedDraft,
            isDirty: true
          });
        } else {
          set(initialState);
        }
      }
    },

    // コンテンツの更新
    updateContent: (content: string) => {
      update(state => ({ ...state, content, isDirty: true }));

      // 自動保存のデバウンス処理
      clearTimeout(saveTimeout);
      saveTimeout = setTimeout(() => {
        const currentState = get(editorStore);
        localStorage.setItem(DRAFT_KEY, JSON.stringify({
          title: currentState.title,
          content: currentState.content,
          tags: currentState.tags
        }));
      }, 1000);
    },

    // タイトルの更新
    updateTitle: (title: string) => {
      update(state => ({ ...state, title, isDirty: true }));
    },

    // タグの更新
    updateTags: (tags: string[]) => {
      update(state => ({ ...state, tags, isDirty: true }));
    },

    // 保存処理
    save: async () => {
      update(state => ({ ...state, isSaving: true }));
      
      try {
        const currentState = get(editorStore);
        const saveData = {
          title: currentState.title,
          content: currentState.content,
          tags: currentState.tags
        };

        let savedMemo: Memo;
        if (currentState.id) {
          savedMemo = await memoApi.updateMemo(currentState.id, saveData);
        } else {
          savedMemo = await memoApi.createMemo(saveData);
        }

        update(state => ({
          ...state,
          id: savedMemo.id,
          isSaving: false,
          isDirty: false,
          lastSavedAt: new Date(savedMemo.updated_at)
        }));

        // 保存成功後は下書きを削除
        localStorage.removeItem(DRAFT_KEY);

        return savedMemo;
      } catch (error) {
        update(state => ({ ...state, isSaving: false }));
        throw error;
      }
    }
  };
}

export const editorStore = createEditorStore();

// 派生ストア：保存可能な状態かどうか
export const canSave = derived(
  editorStore,
  $editor => $editor.isDirty && !$editor.isSaving && $editor.title.trim().length > 0
);