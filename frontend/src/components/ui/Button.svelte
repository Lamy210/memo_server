<!-- /app/src/components/ui/Button.svelte -->
<script lang="ts">
  /**
   * ★ポイント3：'ui.ts' に export されている型を正しくインポート
   * ファイル末尾を確認し、`export type ButtonProps = {...}` を定義してください
   */
  import type { ButtonProps } from '@/lib/types/ui';

  // ★onclick = (event: MouseEvent) => void; を追加で受け取る
  const props = $props<ButtonProps & {
    href?: string;
    onclick?: (event: MouseEvent) => void;
    children?: () => unknown;
  }>();

  const baseStyles =
    'inline-flex items-center justify-center rounded-lg font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2';

  const variants = {
    primary: 'bg-blue-600 text-white hover:bg-blue-700 focus:ring-blue-500',
    secondary: 'bg-gray-600 text-white hover:bg-gray-700 focus:ring-gray-500',
    outline: 'border border-gray-300 bg-white text-gray-700 hover:bg-gray-50 focus:ring-blue-500',
  } as const;

  const sizes = {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-base',
    lg: 'px-6 py-3 text-lg',
  } as const;

  // variant, size を変数にキャストして安全にインデックスアクセス
  const variantKey = (props.variant ?? 'primary') as keyof typeof variants;
  const sizeKey = (props.size ?? 'md') as keyof typeof sizes;

  const className = $derived(`
    ${baseStyles}
    ${variants[variantKey]}
    ${sizes[sizeKey]}
    ${props.fullWidth ? 'w-full' : ''}
    ${props.disabled ? 'opacity-50 cursor-not-allowed' : ''}
    ${props.class ?? ''}
  `.trim());

  function handleClick(event: MouseEvent) {
    if (!props.disabled && props.onclick) {
      props.onclick(event);
    }
  }

  const ariaAttributes = $derived({
    role: 'button',
    'aria-disabled': props.disabled ? 'true' : undefined,
  });
</script>

{#if props.href}
  <!-- Svelte 5 では on:click は非推奨。onclick={...} を使用 -->
  <a
    href={props.href}
    class={className}
    onclick={handleClick}
    {...ariaAttributes}
    {...(props.rest ?? {})}
  >
    {@render props.children?.()}
  </a>
{:else}
  <button
    type={props.type ?? 'button'}
    disabled={props.disabled}
    class={className}
    onclick={handleClick}
    {...ariaAttributes}
    {...(props.rest ?? {})}
  >
    {@render props.children?.()}
  </button>
{/if}
