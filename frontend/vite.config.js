import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig, loadEnv } from 'vite';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '.', '');

  return {
    plugins: [sveltekit()],
    server: {
      host: '0.0.0.0',
      proxy: {
        '/api': {
          target: env.BACKEND_URL || 'http://127.0.0.1:8080',
          changeOrigin: true
        }
      }
    },
    optimizeDeps: {
      include: ['marked', 'dompurify']
    }
  };
});
