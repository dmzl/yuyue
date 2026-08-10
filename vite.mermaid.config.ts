import { defineConfig } from 'vite'

export default defineConfig({
  build: {
    outDir: 'dist',
    emptyOutDir: false,
    rollupOptions: {
      input: new URL('./src/mermaid-renderer-entry.ts', import.meta.url).pathname,
      output: {
        format: 'iife',
        inlineDynamicImports: true,
        entryFileNames: 'assets/mermaid-renderer-[hash].js',
      },
    },
  },
})
