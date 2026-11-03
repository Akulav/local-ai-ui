// Copies the browser builds of the UI libraries from node_modules into ui/vendor.
// The UI has no bundler: ui/ is served as-is by Tauri, so vendored files are committed.
// Run after upgrading a dependency:  npm run vendor
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..");
const out = path.join(root, "ui", "vendor");
const files = [
  ["node_modules/marked/lib/marked.umd.js", "marked.umd.js"],
  ["node_modules/dompurify/dist/purify.min.js", "purify.min.js"],
  ["node_modules/@highlightjs/cdn-assets/highlight.min.js", "highlight.min.js"],
  ["node_modules/@highlightjs/cdn-assets/styles/github-dark.min.css", "hljs-dark.css"],
];

fs.mkdirSync(out, { recursive: true });
for (const [from, to] of files) {
  fs.copyFileSync(path.join(root, from), path.join(out, to));
  console.log(`vendored ${to}`);
}
