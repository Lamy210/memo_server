<!-- src/components/ui/Card.svelte -->
<script lang="ts">
  const props = $props<{
    padding?: 'none' | 'sm' | 'md' | 'lg';
    hover?: boolean;
    class?: string;
    rest?: Record<string, any>;
  }>();

  const paddings = {
    none: '',
    sm: 'p-3',
    md: 'p-4',
    lg: 'p-6'
  };

  const className = $derived(`
    bg-white rounded-lg shadow-sm
    ${paddings[props.padding ?? 'md']}
    ${props.hover ? 'hover:shadow-md transition-shadow' : ''}
    ${props.class ?? ''}
  `);
</script>

<div class={className} {...props.rest}>
  <slot />
</div>