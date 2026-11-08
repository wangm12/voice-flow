import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";

function findProjectRoot(start) {
  let directory = path.resolve(start);
  while (directory !== path.dirname(directory)) {
    if (existsSync(path.join(directory, "node_modules", "vite"))) return directory;
    directory = path.dirname(directory);
  }
  throw new Error("Could not find the VoiceFlow dependency tree above this mockup.");
}

export default function createConfig(configUrl) {
  const root = path.dirname(fileURLToPath(configUrl));
  const projectRoot = findProjectRoot(root);
  const resolveDependency = (name) => path.join(projectRoot, "node_modules", name);

  return {
    root,
    configFile: false,
    define: { "process.env.NODE_ENV": JSON.stringify("production") },
    plugins: [react()],
    resolve: {
      alias: [
        { find: /^react$/, replacement: resolveDependency("react/index.js") },
        { find: /^react-dom$/, replacement: resolveDependency("react-dom/index.js") },
        { find: /^react\/jsx-runtime$/, replacement: resolveDependency("react/jsx-runtime.js") },
        { find: /^react-dom\/client$/, replacement: resolveDependency("react-dom/client.js") },
        { find: /^border-beam$/, replacement: resolveDependency("border-beam/dist/index.es.js") },
        { find: /^thinking-orbs$/, replacement: resolveDependency("thinking-orbs/dist/index.es.js") },
      ],
    },
    cacheDir: path.join(root, ".vite-cache"),
    server: {
      host: "127.0.0.1",
      port: 4184,
      strictPort: true,
      fs: { allow: [root, path.join(projectRoot, "node_modules")] },
    },
    build: {
      outDir: path.join(root, ".inline-build"),
      emptyOutDir: true,
      cssCodeSplit: false,
      lib: {
        entry: path.join(root, "src/main.tsx"),
        name: "VoiceHudBeamMockup",
        cssFileName: "mockup",
        formats: ["iife"],
        fileName: () => "mockup.iife.js",
      },
      rollupOptions: {
        output: { inlineDynamicImports: true },
      },
    },
  };
}
