import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://localhost:3000',
        ws: true,
      },
    },
  },
  build: {
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules/uplot')) return 'uplot';
          if (id.includes('node_modules/apache-arrow')) return 'arrow';
          if (id.includes('node_modules/@dnd-kit')) return 'dnd';
          if (id.includes('node_modules/lucide-react')) return 'icons';
        },
      },
    },
  },
})
