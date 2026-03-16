import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  server: {
    proxy: {
      "/events": "http://127.0.0.1:8080",
      "/refresh": "http://127.0.0.1:8080",
    },
  },
  plugins: [solid()],
});
