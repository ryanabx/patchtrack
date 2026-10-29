import preact from "@preact/preset-vite";
import { defineConfig, type Plugin } from "vite";

// The production page's Content-Security-Policy: everything shown is mail,
// so nothing but this site's own scripts, styles and data.
// Connections: this site, this computer (`patchtrack bridge`), and https
// anywhere, for a bridge reached from elsewhere (its --public-url), which
// the app pairs with only once you confirm it.
// (The dev server injects its own scripts, so only builds get it.)
const csp: Plugin = {
  name: "patchtrack-csp",
  apply: "build",
  transformIndexHtml: (html) =>
    html.replace(
      "<head>",
      `<head>\n    <meta http-equiv="Content-Security-Policy" content="default-src 'self'; ` +
        `script-src 'self'; img-src 'self' data:; ` +
        `connect-src 'self' http://127.0.0.1:* http://localhost:* https:; ` +
        `base-uri 'none'; form-action 'none'">`,
    ),
};

export default defineConfig({
  // Relative URLs, so the build works from any directory, e.g. a GitHub Pages project site.
  base: "./",
  plugins: [preact(), csp],
  build: { target: "es2022" },
});
