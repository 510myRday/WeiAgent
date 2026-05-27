import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, '..', '');
  const host = env.WEIAGENT_HOST === '0.0.0.0' || env.WEIAGENT_HOST === '::'
    ? '127.0.0.1'
    : (env.WEIAGENT_HOST || '127.0.0.1');
  const port = env.WEIAGENT_PORT || '8080';

  return {
    envDir: '..',
    define: {
      __WEIAGENT_API_BASE_URL__: JSON.stringify(''),
    },
    plugins: [react],
    server: {
      proxy: {
        '/api': {
          target: 'http://' + host + ':' + port,
          changeOrigin: true,
        },
      },
    },
  };
});
