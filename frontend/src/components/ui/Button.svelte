<!-- src/components/ui/Button.svelte -->
<script lang="ts">
  // プロパティの型定義（子要素のレンダリング関数を含む）
  const props = $props<{
    variant?: 'primary' | 'secondary' | 'outline';
    size?: 'sm' | 'md' | 'lg';
    disabled?: boolean;
    type?: 'button' | 'submit' | 'reset';
    fullWidth?: boolean;
    class?: string;
    onclick?: (event: MouseEvent) => void;
    rest?: Record<string, any>;
    children?: () => unknown;  // 子要素のレンダリング関数
  }>();

  // スタイル定数の定義
  const baseStyles = 'inline-flex items-center justify-center rounded-lg font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2';
  
  // バリアント定義（as constによる型の厳密化）
  const variants = {
    primary: 'bg-blue-600 text-white hover:bg-blue-700 focus:ring-blue-500',
    secondary: 'bg-gray-600 text-white hover:bg-gray-700 focus:ring-gray-500',
    outline: 'border border-gray-300 bg-white text-gray-700 hover:bg-gray-50 focus:ring-blue-500'
  } as const;
  
  // サイズ定義（as constによる型の厳密化）
  const sizes = {
    sm: 'px-3 py-1.5 text-sm',
    md: 'px-4 py-2 text-base',
    lg: 'px-6 py-3 text-lg'
  } as const;

  // クラス名の動的生成（メモ化による最適化）
  const className = $derived(`
    ${baseStyles}
    ${variants[props.variant ?? 'primary']}
    ${sizes[props.size ?? 'md']}
    ${props.fullWidth ? 'w-full' : ''}
    ${props.disabled ? 'opacity-50 cursor-not-allowed' : ''}
    ${props.class ?? ''}
  `.trim());

  // クリックイベントハンドラ
  function handleClick(event: MouseEvent) {
    if (!props.disabled && props.onclick) {
      props.onclick(event);
    }
  }

  // WAI-ARIA属性の設定
  const ariaAttributes = $derived({
    role: 'button',
    'aria-disabled': props.disabled ? 'true' : undefined
  });
</script>

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