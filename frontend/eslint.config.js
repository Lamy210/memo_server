import prettier from 'eslint-config-prettier';
import js from '@eslint/js';
import { includeIgnoreFile } from '@eslint/compat';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import { fileURLToPath } from 'node:url';

const gitignorePath = fileURLToPath(new URL('./.gitignore', import.meta.url));

/** @type {import('eslint').Linter.Config[]} */
export default [
  includeIgnoreFile(gitignorePath),
  js.configs.recommended,
  ...svelte.configs['flat/recommended'],
  prettier,
  ...svelte.configs['flat/prettier'],
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node
      }
    },
    settings: {
      'import/resolver': {
        alias: {
          map: [
            ['@', './src'],
            ['@components', './src/components'],
            ['@features', './src/components/features'],
            ['@ui', './src/components/ui'],
            ['@lib', './src/lib'],
            ['@api', './src/lib/api'],
            ['@stores', './src/lib/stores'],
            ['@utils', './src/lib/utils'],
            ['@styles', './src/styles'],
            ['@types', './src/types']
          ],
          extensions: ['.ts', '.js', '.svelte']
        }
      }
    },
    rules: {
      'svelte/valid-compile': ['error'],
      'svelte/no-at-html-tags': 'warn',
      'svelte/require-store-callbacks-use-set-param': ['error'],
      'svelte/require-store-reactive-access': ['error']
    }
  }
];