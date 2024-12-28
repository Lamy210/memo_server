<!-- src/components/ui/Card.svelte -->
<script lang="ts">
    export let padding: 'none' | 'sm' | 'md' | 'lg' = 'md';
    export let hover = false;
  
    const paddings = {
      none: '',
      sm: 'p-3',
      md: 'p-4',
      lg: 'p-6'
    };
  
    $: className = `
      bg-white rounded-lg shadow-sm
      ${paddings[padding]}
      ${hover ? 'hover:shadow-md transition-shadow' : ''}
    `;
  </script>
  
  <div class={className} {...$$restProps}>
    <slot />
  </div>