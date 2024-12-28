// src/lib/utils/mime.ts
export type FileType = 'image' | 'pdf' | 'text' | 'code' | 'video' | 'audio' | 'other';

export function getFileType(mimeType: string): FileType {
  if (mimeType.startsWith('image/')) return 'image';
  if (mimeType === 'application/pdf') return 'pdf';
  if (mimeType.startsWith('text/')) return 'text';
  if (isCodeFile(mimeType)) return 'code';
  if (mimeType.startsWith('video/')) return 'video';
  if (mimeType.startsWith('audio/')) return 'audio';
  return 'other';
}

export function isCodeFile(mimeType: string): boolean {
  const codeExtensions = [
    'javascript', 'typescript', 'python', 'java', 'ruby', 'php',
    'c', 'cpp', 'cs', 'go', 'rust', 'swift', 'kotlin'
  ];
  
  return codeExtensions.some(ext => 
    mimeType === `text/x-${ext}` || 
    mimeType === `application/x-${ext}`
  );
}

// プレビュー可能なファイルサイズの上限（5MB）
export const PREVIEW_SIZE_LIMIT = 5 * 1024 * 1024;

// プレビュー可能かどうかを判定
export function isPreviewable(attachment: Attachment): boolean {
  const fileType = getFileType(attachment.mime_type);
  return (
    attachment.size <= PREVIEW_SIZE_LIMIT &&
    (fileType === 'image' || fileType === 'pdf' || fileType === 'text' || fileType === 'code')
  );
}