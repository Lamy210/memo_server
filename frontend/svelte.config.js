import adapter from '@sveltejs/adapter-node';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  kit: {
    adapter: adapter(),
    csp: {
      mode: 'auto',
      directives: {
        'base-uri': ['self'],
        'connect-src': ['self'],
        'default-src': ['self'],
        'font-src': ['self'],
        'form-action': ['self'],
        'frame-ancestors': ['none'],
        'img-src': ['self', 'data:'],
        'manifest-src': ['self'],
        'object-src': ['none'],
        'script-src': ['self'],
        'style-src': ['self', 'unsafe-inline'],
        'worker-src': ['self']
      }
    },
    alias: {
      '@': 'src',
      '@lib': 'src/lib',
      '@utils': 'src/lib/utils',
      '@components': 'src/components'
    }
  },
  preprocess: vitePreprocess()
};

export default config;
