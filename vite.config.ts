import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// macOS-only app: the WKWebView shipped with macOS 26 supports modern syntax,
// so we can target a recent baseline and skip legacy transpilation.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    css: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
});
