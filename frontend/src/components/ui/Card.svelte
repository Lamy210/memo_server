<!-- /app/src/components/ui/Card.svelte -->
<script lang="ts">
  // ★ポイント4：paddingを変数で取り出し、型を絞ってインデックスアクセス
  const props = $props<{
    padding?: 'none' | 'sm' | 'md' | 'lg';
    hover?: boolean;
    class?: string;
    rest?: Record<string, any>;
    children?: () => unknown;
  }>();

  const paddings = {
    none: '',
    sm: 'p-3',
    md: 'p-4',
    lg: 'p-6',
  } as const;

  // 安全にアクセスするため、型を keyof typeof paddings にキャスト
  const paddingKey = (props.padding ?? 'md') as keyof typeof paddings;

  const className = $derived(`
    bg-white rounded-lg shadow-sm
    ${paddings[paddingKey]}
    ${props.hover ? 'hover:shadow-md transition-shadow' : ''}
    ${props.class ?? ''}
  `);
</script>

<div class={className} {...props.rest}>
  {@render props.children?.()}
</div>
