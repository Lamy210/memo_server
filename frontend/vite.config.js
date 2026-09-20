import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit(), svelteTesting()],
  server: {
    host: '0.0.0.0'
  },
  optimizeDeps: {
    include: ['marked', 'dompurify']
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts']
  }
});
