import { defineConfig } from "vite";

export default defineConfig({
  optimizeDeps: {
    exclude: ["@ragelab/wasm"],
  },
  build: {
    target: "es2022",
  },
});
