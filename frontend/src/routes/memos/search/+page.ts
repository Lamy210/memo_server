import type { PageLoad } from './$types';

export const load: PageLoad = ({ url }) => ({
  query: url.searchParams.get('query') ?? '',
  tag: url.searchParams.get('tag') ?? '',
  page: Math.max(1, Number(url.searchParams.get('page') ?? '1') || 1)
});
