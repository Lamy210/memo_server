// src/lib/api/types/search.ts
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
    totalPages: number;
  }