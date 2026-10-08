export interface Memo {
  id: string;
  title: string;
  content: string;
  tags: string[];
  user_id: string;
  created_at: string;
  updated_at: string;
  version: number;
}

export interface CreateMemoInput {
  title: string;
  content: string;
  tags: string[];
}

export interface UpdateMemoInput {
  title?: string;
  content?: string;
  tags?: string[];
  version: number;
}

export interface MemoListParams {
  cursor?: string;
  limit?: number;
}

export interface MemoListPage {
  pagination: 'cursor-v1';
  items: Memo[];
  limit: number;
  next_cursor: string | null;
}

export interface SearchParams {
  query?: string;
  tag?: string;
  page?: number;
  limit?: number;
}

export interface SearchResult<T> {
  items: T[];
  total: number;
  page: number;
  total_pages: number;
}
