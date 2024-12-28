import adapter from '@sveltejs/adapter-node';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    adapter: adapter(),
    alias: {
      '@': 'src',
      '@lib': 'src/lib',
      '@utils': 'src/lib/utils',
      '@components': 'src/components'
    }
  },
  
  preprocess: vitePreprocess(),
  
  compilerOptions: {
    runes: true
  }
};

export default config;