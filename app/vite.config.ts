import { defineConfig } from "vite";

// Tauri expects a fixed port and must not see its own Rust sources change.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  // Two pages: the app and the startup window (splash.html).
  build: { target: "es2022", rollupOptions: { input: { main: "index.html", splash: "splash.html" } } },
});
