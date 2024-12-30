// src/routes/memos/search/+page.ts
import { error } from '@sveltejs/kit';
import type { PageLoad } from '@sveltejs/kit';
import * as memoApi from '@/lib/api/memo';

export const load = (async ({ url }) => {
  const query = url.searchParams.get('q') || '';
  const tag = url.searchParams.get('tag');

  try {
    const searchResult = await memoApi.searchMemos({ 
      query, 
      tag,
      page: 1,
      limit: 20
    });

    return {
      memos: searchResult.items,
      searchParams: { query, tag }
    };
  } catch (e) {
    throw error(500, {
      message: '検索に失敗しました'
    });
  }
}) satisfies PageLoad;