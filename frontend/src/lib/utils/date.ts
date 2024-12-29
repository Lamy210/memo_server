/**
 * 日付操作ユーティリティ関数群
 */
export interface DateFormatOptions extends Intl.DateTimeFormatOptions {
  locale?: string;
}

/**
 * 日付を指定されたフォーマットで文字列に変換
 */
export function formatDate(
  date: Date | string | number,
  options: DateFormatOptions = {}
): string {
  try {
    const targetDate = date instanceof Date ? date : new Date(date);
    
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

    return new Intl.DateTimeFormat(defaultOptions.locale, defaultOptions).format(targetDate);
  } catch (error) {
    console.error('日付フォーマットエラー:', error);
    return '日付フォーマットエラー';
  }
}