import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The Tauri dev server must run on the fixed port `devUrl` points at.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Ignore the Rust host's rebuilds so HMR does not restart.
      ignored: ["**/src-tauri/**"],
    },
  },
});
