import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import path from 'path';

export default defineConfig({
  plugins: [sveltekit()],
  
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      '@lib': path.resolve(__dirname, './src/lib'),
      '@utils': path.resolve(__dirname, './src/lib/utils'),
      '@components': path.resolve(__dirname, './src/components')
    }
  },

  server: {
    fs: {
      allow: ['..']
    }
  },

  optimizeDeps: {
    include: ['marked', 'dompurify', 'prismjs']
  }
});