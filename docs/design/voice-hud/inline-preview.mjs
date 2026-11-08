import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
const output = path.join(root, ".inline-build");
const files = await readdir(output);
const jsName = files.find((file) => file.endsWith(".js"));
const cssName = files.find((file) => file.endsWith(".css"));
if (!jsName || !cssName) throw new Error("Expected one JavaScript bundle and one CSS bundle in .inline-build");

const [js, css] = await Promise.all([
  readFile(path.join(output, jsName), "utf8"),
  readFile(path.join(output, cssName), "utf8"),
]);
const safeJs = js.replace(/<\/script/gi, "<\\/script");
const html = `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <meta name="color-scheme" content="light dark">
  <title>VoiceFlow · 录音 HUD 比较原型</title>
  <style>${css}</style>
</head>
<body>
  <div id="root"></div>
  <script>${safeJs}</script>
</body>
</html>
`;
await writeFile(path.join(root, "preview.html"), html, { mode: 0o600 });
console.log(`Wrote ${path.join(root, "preview.html")} (${Buffer.byteLength(html)} bytes)`);
