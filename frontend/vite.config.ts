import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

export default defineConfig(({ command }) => ({
  plugins: [react],
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
