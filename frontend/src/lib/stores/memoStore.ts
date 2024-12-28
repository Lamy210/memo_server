// src/lib/stores/memoStore.ts
import { writable } from 'svelte/store';
import type { Memo } from '../api/types';
import * as memoApi from '../api/memo';

function createMemoStore() {
  const { subscribe, set, update } = writable<Memo[]>([]);

  return {
    subscribe,
    fetchAll: async () => {
      try {
        const memos = await memoApi.fetchMemos();
        set(memos);
      } catch (error) {
        console.error('Failed to fetch memos:', error);
        set([]);
      }
    },
    add: async (data: Omit<Memo, 'id' | 'created_at' | 'updated_at'>) => {
      try {
        const newMemo = await memoApi.createMemo(data);
        update(memos => [...memos, newMemo]);
        return newMemo;
      } catch (error) {
        console.error('Failed to create memo:', error);
        throw error;
      }
    },
    update: async (id: string, data: Partial<Memo>) => {
      try {
        const updatedMemo = await memoApi.updateMemo(id, data);
        update(memos => memos.map(memo => 
          memo.id === id ? updatedMemo : memo
        ));
        return updatedMemo;
      } catch (error) {
        console.error('Failed to update memo:', error);
        throw error;
      }
    }
  };
}

export const memos = createMemoStore();