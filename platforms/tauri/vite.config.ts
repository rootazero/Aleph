import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: ['es2021', 'chrome100', 'safari15'],
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    chunkSizeWarningLimit: 500,
    rollupOptions: {
      input: {
        main: path.resolve(__dirname, 'index.html'),
        halo: path.resolve(__dirname, 'halo.html'),
        settings: path.resolve(__dirname, 'settings.html'),
      },
      output: {
        manualChunks: {
          // Vendor chunks for better caching
          'react-vendor': ['react', 'react-dom'],
          'ui-vendor': ['framer-motion', 'lucide-react'],
          'radix-vendor': [
            '@radix-ui/react-switch',
            '@radix-ui/react-select',
            '@radix-ui/react-slider',
            '@radix-ui/react-dialog',
            '@radix-ui/react-slot',
          ],
          'tauri-vendor': [
            '@tauri-apps/api',
            '@tauri-apps/plugin-dialog',
            '@tauri-apps/plugin-fs',
            '@tauri-apps/plugin-global-shortcut',
            '@tauri-apps/plugin-notification',
            '@tauri-apps/plugin-shell',
          ],
          'state': ['zustand'],
          'i18n': ['i18next', 'react-i18next'],
        },
      },
    },
  },
  // Optimize deps
  optimizeDeps: {
    include: [
      'react',
      'react-dom',
      'framer-motion',
      'zustand',
      'lucide-react',
    ],
  },
});
