import adapter from '@sveltejs/adapter-node';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    // アダプター設定
    adapter: adapter(),
    
    // エイリアスの定義
    alias: {
      '@': 'src',
      '@components': 'src/components',
      '@features': 'src/components/features',
      '@ui': 'src/components/ui',
      '@lib': 'src/lib',
      '@api': 'src/lib/api',
      '@stores': 'src/lib/stores',
      '@utils': 'src/lib/utils',
      '@styles': 'src/styles',
      '@types': 'src/types'
    }
  },
  
  // プリプロセッサの設定
  preprocess: vitePreprocess()
};

export default config;