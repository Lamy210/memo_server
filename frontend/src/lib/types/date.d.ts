// src/lib/types/date.d.ts

declare module '@/lib/utils/date' {
    export interface DateFormatOptions extends Intl.DateTimeFormatOptions {
      locale?: string;
    }
  
    export function formatDate(
      date: Date | string | number,
      options?: DateFormatOptions
    ): string;
  
    export function getRelativeTimeString(
      date: Date | string | number
    ): string;
  
    export function isValidDate(
      date: Date | string | number
    ): boolean;
  
    export function toISOString(
      date: Date | string | number
    ): string;
  }