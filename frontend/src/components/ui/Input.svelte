<!-- /app/src/components/ui/Input.svelte -->
<script lang="ts">
  let {
    type = 'text',
    placeholder = '',
    label = '',
    error = '',
    required = false,
    class: classNameProp = '',
    rest = {},
    // bind:value する変数は let かつ $bindable()
    value = $bindable('')
  } = $props<{
    type?: 'text' | 'email' | 'password' | 'number';
    value?: string;
    placeholder?: string;
    label?: string;
    error?: string;
    required?: boolean;
    class?: string;
    rest?: Record<string, any>;
  }>();

  const baseStyles =
    'w-full rounded-lg border border-gray-300 px-4 py-2 focus:border-blue-500 focus:ring-2 focus:ring-blue-500 focus:ring-opacity-50 outline-none transition-colors';
  const errorStyles = 'border-red-500 focus:border-red-500 focus:ring-red-500';

  const className = $derived(`
    ${baseStyles}
    ${error ? errorStyles : ''}
    ${classNameProp}
  `);

  let inputId = $state(`input-${crypto.randomUUID()}`);
</script>

{#if label}
  <label for={inputId} class="block text-sm font-medium text-gray-700 mb-1">
    {label}
    {#if required}
      <span class="text-red-500">*</span>
    {/if}
  </label>
{/if}

<input
  id={inputId}
  type={type}
  bind:value={value}
  placeholder={placeholder}
  required={required}
  class={className}
  {...rest}
/>

{#if error}
  <p class="mt-1 text-sm text-red-600">{error}</p>
{/if}
