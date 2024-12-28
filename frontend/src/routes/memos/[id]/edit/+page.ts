// src/routes/memos/[id]/edit/+page.ts
import type { PageLoad } from './$types';
import { error } from '@sveltejs/kit';
import * as memoApi from '@/lib/api/memo';

export const load: PageLoad = async ({ params }) => {
  try {
    const memo = await memoApi.fetchMemoById(params.id);
    return {
      memo
    };
  } catch (e) {
    throw error(404, {
      message: 'メモが見つかりませんでした。'
    });
  }
};