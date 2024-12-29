// src/lib/api/types.ts
export interface Memo {
  id: string;
  title: string;
  content: string;
  tags: string[];
  created_at: string;
  updated_at: string;
  user_id: string;
}
  
  export interface User {
    id: string;
    name: string;
    email: string;
    avatar_url?: string;
  }