import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri 前端构建配置：固定端口，构建产物输出 dist/
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "chrome110",
    minify: "esbuild",
    sourcemap: false,
    outDir: "dist",
  },
});
