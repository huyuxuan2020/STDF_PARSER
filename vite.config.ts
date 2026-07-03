import { defineConfig } from "vite";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import babelPlugin from "@rolldown/plugin-babel";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [
    react(),
    // React Compiler auto-memoizes components, so a state change (context menu,
    // load-more spinner, ...) no longer re-renders the whole test-item matrix.
    // The oxc-based plugin-react has no babel pipeline, so the compiler runs via
    // @rolldown/plugin-babel. target 18: emits react-compiler-runtime calls.
    babelPlugin({
      include: /\.tsx(?:$|\?)/,
      presets: [reactCompilerPreset({ target: "18" })]
    }),
    tailwindcss()
  ],
  clearScreen: false,
  server: {
    strictPort: true,
    port: 1420
  },
  envPrefix: ["VITE_", "TAURI_"],
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts"
  }
});
