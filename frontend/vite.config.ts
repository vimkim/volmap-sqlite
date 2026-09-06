import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  build: {
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: "assets/app.js",
        assetFileNames: ({ names }) =>
          names.some((name) => name.endsWith(".css"))
            ? "assets/app.css"
            : "assets/[name][extname]",
      },
    },
  },
});
