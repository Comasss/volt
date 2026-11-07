import { defineConfig } from "vite";

// Tauri espera un puerto fijo y no queremos que Vite limpie la consola de cargo.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    cssMinify: true,
    // WebView2 es Chromium: no hace falta transpilar a nada antiguo.
    modulePreload: { polyfill: false },
  },
});
