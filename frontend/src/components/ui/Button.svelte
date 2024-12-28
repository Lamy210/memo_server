<!-- src/components/ui/Button.svelte -->
<script lang="ts">
    export let variant: 'primary' | 'secondary' | 'outline' = 'primary';
    export let size: 'sm' | 'md' | 'lg' = 'md';
    export let disabled = false;
    export let type: 'button' | 'submit' | 'reset' = 'button';
    export let fullWidth = false;
  
    const baseStyles = 'inline-flex items-center justify-center rounded-lg font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2';
    
    const variants = {
      primary: 'bg-blue-600 text-white hover:bg-blue-700 focus:ring-blue-500',
      secondary: 'bg-gray-600 text-white hover:bg-gray-700 focus:ring-gray-500',
      outline: 'border border-gray-300 bg-white text-gray-700 hover:bg-gray-50 focus:ring-blue-500'
    };
    
    const sizes = {
      sm: 'px-3 py-1.5 text-sm',
      md: 'px-4 py-2 text-base',
      lg: 'px-6 py-3 text-lg'
    };
  
    $: className = `
      ${baseStyles}
      ${variants[variant]}
      ${sizes[size]}
      ${fullWidth ? 'w-full' : ''}
      ${disabled ? 'opacity-50 cursor-not-allowed' : ''}
    `;
  </script>
  
  <button
    {type}
    {disabled}
    class={className}
    on:click
    {...$$restProps}
  >
    <slot />
  </button>