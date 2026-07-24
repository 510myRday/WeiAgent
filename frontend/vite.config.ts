import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

export default defineConfig(({ command }) => ({
  plugins: [react()],
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          const normalizedId = id.replace(/\\/g, '/');

          if (!normalizedId.includes('node_modules')) {
            return undefined;
          }

          if (normalizedId.includes('katex')) {
            return 'math-vendor';
          }

          if (normalizedId.includes('react-dom') || normalizedId.includes('node_modules/react/')) {
            return 'react-vendor';
          }

          if (
            normalizedId.includes('react-markdown') ||
            normalizedId.includes('remark-') ||
            normalizedId.includes('rehype-') ||
            normalizedId.includes('highlight.js') ||
            normalizedId.includes('micromark') ||
            normalizedId.includes('mdast-util') ||
            normalizedId.includes('hast-util') ||
            normalizedId.includes('unist-util') ||
            normalizedId.includes('vfile') ||
            normalizedId.includes('devlop') ||
            normalizedId.includes('decode-named-character-reference') ||
            normalizedId.includes('comma-separated-tokens') ||
            normalizedId.includes('property-information') ||
            normalizedId.includes('space-separated-tokens') ||
            normalizedId.includes('trim-lines') ||
            normalizedId.includes('web-namespaces') ||
            normalizedId.includes('zwitch')
          ) {
            return 'markdown-vendor';
          }

          if (normalizedId.includes('lucide-react')) {
            return 'icons-vendor';
          }

          return undefined;
        },
      },
    },
  },
  server:
    command === 'serve'
      ? {
          host: '127.0.0.1',
          port: 5173,
          strictPort: true,
          proxy: {
            '/api': {
              target: readApiOrigin(),
              changeOrigin: true,
            },
          },
        }
      : undefined,
}));

type AppSettings = {
  host: string;
  port: number;
};

function readApiOrigin(): string {
  const settings = JSON.parse(readFileSync('../data/settings.json', 'utf-8')) as AppSettings;
  return `http://${settings.host}:${settings.port}`;
}
