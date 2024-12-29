import { writable, type Writable } from 'svelte/store';
import type { Memo } from '../api/types';

/**
 * メモストアの状態インターフェース
 */
interface MemoStoreState {
  items: Memo[];
  isLoading: boolean;
  error: string | null;
}

/**
 * メモストアの初期状態
 */
const initialState: MemoStoreState = {
  items: [],
  isLoading: false,
  error: null
};

/**
 * メモストア作成関数
 * カスタムストアロジックを実装
 */
function createMemoStore() {
  const { subscribe, set, update }: Writable<MemoStoreState> = writable(initialState);

  // 開発用モックデータ
  const mockMemos: Memo[] = [
    {
      id: '1',
      title: 'サンプルメモ',
      content: 'これはテスト用のメモです。',
      tags: ['サンプル', 'テスト'],
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      user_id: 'user1'
    }
  ];

  return {
    subscribe,

    /**
     * メモ一覧の取得
     * @returns {Promise<void>}
     */
    async fetchAll(): Promise<void> {
      update(state => ({ ...state, isLoading: true, error: null }));

      try {
        // TODO: 実際のAPI呼び出しに置き換え
        await new Promise(resolve => setTimeout(resolve, 1000)); // 通信遅延シミュレーション
        update(state => ({
          ...state,
          items: mockMemos,
          isLoading: false
        }));
      } catch (error) {
        console.error('メモの取得に失敗:', error);
        update(state => ({
          ...state,
          error: 'メモの取得に失敗しました',
          isLoading: false
        }));
      }
    },

    /**
     * ストアの状態リセット
     */
    reset(): void {
      set(initialState);
    }
  };
}

// ストアのインスタンスを作成・エクスポート
export const memoStore = createMemoStore();