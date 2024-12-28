/**
 * 日付操作に関するユーティリティ関数群
 * 国際化対応と型安全性を考慮した実装
 */

// 日付フォーマットのオプション型定義
export interface DateFormatOptions extends Intl.DateTimeFormatOptions {
    locale?: string;
  }
  
  /**
   * 日付を指定されたフォーマットで文字列に変換
   * @param date - フォーマット対象の日付
   * @param options - フォーマットオプション
   * @returns フォーマットされた日付文字列
   */
  export function formatDate(
    date: Date | string | number,
    options: DateFormatOptions = {}
  ): string {
    try {
      const targetDate = date instanceof Date ? date : new Date(date);
      
      // 日付の妥当性検証
      if (isNaN(targetDate.getTime())) {
        throw new Error('無効な日付形式');
      }
  
      const defaultOptions: DateFormatOptions = {
        locale: 'ja-JP',
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
        ...options
      };
  
      return new Intl.DateTimeFormat(
        defaultOptions.locale,
        defaultOptions
      ).format(targetDate);
    } catch (error) {
      console.error('日付フォーマットエラー:', error);
      return '日付フォーマットエラー';
    }
  }