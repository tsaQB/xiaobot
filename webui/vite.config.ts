import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

/* The daemon serves `dist/` under a strict Content-Security-Policy
   (`script-src 'self'`), so the build must not emit inline scripts. */
export default defineConfig({
  base: '/',
  plugins: [vue()],
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        /* The backend's CSRF check compares Origin with its own address. */
        configure: (proxy) => {
          proxy.on('proxyReq', (req) => req.setHeader('origin', 'http://127.0.0.1:8787'))
        },
      },
    },
  },
})
