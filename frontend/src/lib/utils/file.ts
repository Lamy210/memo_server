// src/lib/utils/file.ts

/**
 * ファイル操作に関するユーティリティ関数群
 * セキュリティと信頼性を考慮した堅牢な実装を提供
 */

// ファイルサイズの定数定義
export const FILE_SIZE = {
    KB: 1024,
    MB: 1024 * 1024,
    GB: 1024 * 1024 * 1024,
    // アップロード制限値
    MAX_UPLOAD_SIZE: 50 * 1024 * 1024, // 50MB
  } as const;
  
  /**
   * ファイルサイズを人間が読みやすい形式に変換
   * @param bytes - バイト数
   * @param decimals - 小数点以下の桁数（デフォルト: 2）
   * @returns フォーマットされたサイズ文字列
   */
  export function formatFileSize(bytes: number, decimals: number = 2): string {
    if (bytes === 0) return '0 Bytes';
  
    const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const i = Math.floor(Math.log(bytes) / Math.log(k));
  
    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
  }
  
  /**
   * ファイル名から拡張子を抽出
   * @param filename - ファイル名
   * @returns 拡張子（小文字）
   */
  export function getFileExtension(filename: string): string {
    return filename.slice((filename.lastIndexOf('.') - 1 >>> 0) + 2).toLowerCase();
  }
  
  /**
   * ファイルのMIMEタイプを検証
   * @param file - 検証対象のFile
   * @param allowedTypes - 許可されたMIMEタイプの配列
   * @returns 検証結果
   */
  export function validateFileType(file: File, allowedTypes: string[]): boolean {
    // MIMEタイプの厳密な検証
    const fileType = file.type.toLowerCase();
    return allowedTypes.some(type => {
      // ワイルドカード対応（例: image/*）
      if (type.endsWith('/*')) {
        const baseType = type.slice(0, -2);
        return fileType.startsWith(baseType);
      }
      return fileType === type.toLowerCase();
    });
  }
  
  /**
   * ファイル名のサニタイズ処理
   * 安全なファイル名に変換
   * @param filename - 元のファイル名
   * @returns サニタイズされたファイル名
   */
  export function sanitizeFileName(filename: string): string {
    // 拡張子を保持しつつ、ファイル名を安全な形式に変換
    const extension = getFileExtension(filename);
    const baseName = filename.slice(0, filename.lastIndexOf('.'));
    
    // 危険な文字を除去し、安全な文字のみを許可
    const sanitized = baseName
      .replace(/[^a-zA-Z0-9-_]/g, '_') // 英数字、ハイフン、アンダースコアのみ許可
      .replace(/_{2,}/g, '_')          // 連続するアンダースコアを1つに
      .replace(/^_|_$/g, '');          // 先頭と末尾のアンダースコアを除去
  
    return `${sanitized}.${extension}`;
  }
  
  /**
   * ファイルのチャンク分割
   * 大容量ファイルの分割アップロード用
   * @param file - 分割対象のFile
   * @param chunkSize - チャンクサイズ（バイト）
   * @returns チャンクの配列
   */
  export function createFileChunks(file: File, chunkSize: number = 1024 * 1024): Blob[] {
    const chunks: Blob[] = [];
    let start = 0;
  
    while (start < file.size) {
      const end = Math.min(start + chunkSize, file.size);
      chunks.push(file.slice(start, end));
      start = end;
    }
  
    return chunks;
  }
  
  /**
   * ファイルのハッシュ値を計算
   * ファイルの一意性確認やインテグリティチェック用
   * @param file - ハッシュ値を計算するFile
   * @returns SHA-256ハッシュ値
   */
  export async function calculateFileHash(file: File): Promise<string> {
    const buffer = await file.arrayBuffer();
    const hashBuffer = await crypto.subtle.digest('SHA-256', buffer);
    const hashArray = Array.from(new Uint8Array(hashBuffer));
    return hashArray.map(b => b.toString(16).padStart(2, '0')).join('');
  }
  
  /**
   * Base64エンコードされたデータをBlobに変換
   * @param base64 - Base64文字列
   * @param mimeType - MIMEタイプ
   * @returns Blobオブジェクト
   */
  export function base64ToBlob(base64: string, mimeType: string): Blob {
    const byteCharacters = atob(base64.split(',')[1]);
    const byteNumbers = new Array(byteCharacters.length);
    
    for (let i = 0; i < byteCharacters.length; i++) {
      byteNumbers[i] = byteCharacters.charCodeAt(i);
    }
    
    const byteArray = new Uint8Array(byteNumbers);
    return new Blob([byteArray], { type: mimeType });
  }
  
  /**
   * ファイルのプレビューURLを生成
   * @param file - プレビュー対象のFile
   * @returns プレビューURL（ObjectURL）
   */
  export function createFilePreviewUrl(file: File): string {
    return URL.createObjectURL(file);
  }
  
  /**
   * ObjectURLの解放
   * メモリリーク防止のため、不要になったObjectURLを解放
   * @param url - 解放するObjectURL
   */
  export function revokeFilePreviewUrl(url: string): void {
    URL.revokeObjectURL(url);
  }
  
  /**
   * ファイルダウンロードの実行
   * @param url - ダウンロードURL
   * @param filename - 保存するファイル名
   */
  export function downloadFile(url: string, filename: string): void {
    const link = document.createElement('a');
    link.href = url;
    link.download = sanitizeFileName(filename);
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }