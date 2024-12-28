// src/lib/types/attachment.ts
export interface Attachment {
    id: string;
    filename: string;
    size: number;
    mime_type: string;
    created_at: string;
    memo_id: string;
    url: string;
  }
  
  export interface UploadProgress {
    fileId: string;
    progress: number;
    status: 'pending' | 'uploading' | 'completed' | 'error';
    error?: string;
  }