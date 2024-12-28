<!-- src/components/ui/Input.svelte -->
<script lang="ts">
    export let type: 'text' | 'email' | 'password' | 'number' = 'text';
    export let value: string = '';
    export let placeholder: string = '';
    export let label: string = '';
    export let error: string = '';
    export let required = false;
  
    const baseStyles = 'w-full rounded-lg border border-gray-300 px-4 py-2 focus:border-blue-500 focus:ring-2 focus:ring-blue-500 focus:ring-opacity-50 outline-none transition-colors';
    const errorStyles = 'border-red-500 focus:border-red-500 focus:ring-red-500';
  
    $: className = `
      ${baseStyles}
      ${error ? errorStyles : ''}
    `;
  </script>
  
  {#if label}
    <label class="block text-sm font-medium text-gray-700 mb-1">
      {label}
      {#if required}
        <span class="text-red-500">*</span>
      {/if}
    </label>
  {/if}
  
  <input
    {type}
    bind:value
    {placeholder}
    {required}
    class={className}
    {...$$restProps}
  />
  
  {#if error}
    <p class="mt-1 text-sm text-red-600">{error}</p>
  {/if}