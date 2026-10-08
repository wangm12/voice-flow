import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

// Shared HUD sources resolve against the website's own dependency installation.
const localDependency = (path: string) =>
  fileURLToPath(new URL(`./node_modules/${path}`, import.meta.url));

export default defineConfig({
  plugins: [react()],
  base: "./",
  resolve: {
    alias: [
      { find: "react", replacement: localDependency("react") },
      { find: "react-dom", replacement: localDependency("react-dom") },
      {
        find: "thinking-orbs",
        replacement: localDependency("thinking-orbs/dist/index.es.js"),
      },
      {
        find: "border-beam",
        replacement: localDependency("border-beam/dist/index.es.js"),
      },
    ],
  },
  server: { port: 5173, strictPort: true },
  preview: { port: 4173, strictPort: true },
});
