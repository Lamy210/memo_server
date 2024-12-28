import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import path from 'path';

export default defineConfig({
  plugins: [sveltekit()],
  
  resolve: {
    alias: {
      '@': path.resolve('./src'),
      '@components': path.resolve('./src/components'),
      '@features': path.resolve('./src/components/features'),
      '@ui': path.resolve('./src/components/ui'),
      '@lib': path.resolve('./src/lib'),
      '@api': path.resolve('./src/lib/api'),
      '@stores': path.resolve('./src/lib/stores'),
      '@utils': path.resolve('./src/lib/utils'),
      '@styles': path.resolve('./src/styles'),
      '@types': path.resolve('./src/types')
    }
  },
  
  server: {
    port: 3000,
    strictPort: false,
    host: true,
    fs: {
      strict: false,
      allow: ['..']
    },
    watch: {
      usePolling: false,
      interval: 100
    }
  },
  
  build: {
    target: 'esnext',
    sourcemap: true,
    outDir: 'build',
    assetsDir: 'assets',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: undefined
      }
    }
  }
});