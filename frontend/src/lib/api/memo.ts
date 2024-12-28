// src/lib/api/memo.ts
import type { Memo } from './types';

const API_BASE = '/api/v1';

export async function fetchMemos(): Promise<Memo[]> {
  const response = await fetch(`${API_BASE}/memos`);
  if (!response.ok) {
    throw new Error('Failed to fetch memos');
  }
  return response.json();
}

export async function fetchMemoById(id: string): Promise<Memo> {
  const response = await fetch(`${API_BASE}/memos/${id}`);
  if (!response.ok) {
    throw new Error('Failed to fetch memo');
  }
  return response.json();
}

export async function createMemo(data: Omit<Memo, 'id' | 'created_at' | 'updated_at'>): Promise<Memo> {
  const response = await fetch(`${API_BASE}/memos`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(data),
  });
  if (!response.ok) {
    throw new Error('Failed to create memo');
  }
  return response.json();
}

export async function updateMemo(id: string, data: Partial<Memo>): Promise<Memo> {
  const response = await fetch(`${API_BASE}/memos/${id}`, {
    method: 'PATCH',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(data),
  });
  if (!response.ok) {
    throw new Error('Failed to update memo');
  }
  return response.json();
}