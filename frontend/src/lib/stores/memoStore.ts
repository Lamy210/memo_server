// src/lib/stores/memoStore.ts
import { writable, type Writable } from 'svelte/store';
import type { Memo } from '../api/types';

interface MemoStoreState {
  items: Memo[];
  isLoading: boolean;
  error: string | null;
}

const initialState: MemoStoreState = {
  items: [],
  isLoading: false,
  error: null
};

function createMemoStore() {
  const { subscribe, set, update }: Writable<MemoStoreState> = writable(initialState);

  return {
    subscribe,

    async fetchAll(): Promise<void> {
      update(state => ({ ...state, isLoading: true, error: null }));

      try {
        // TODO: 実際のAPI実装に置き換え
        const response = await fetch('/api/v1/memos');
        if (!response.ok) {
          throw new Error('Failed to fetch memos');
        }
        const memos = await response.json();
        
        update(state => ({
          ...state,
          items: memos,
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

    reset(): void {
      set(initialState);
    }
  };
}

export const memoStore = createMemoStore();