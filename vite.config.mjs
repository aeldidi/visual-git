import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/events": "http://127.0.0.1:8080",
      "/command": "http://127.0.0.1:8080",
    },
  },
  build: {
    outDir: "ui-dist",
    emptyOutDir: true,
  },
});
