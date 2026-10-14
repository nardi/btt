// Serves the built viewer under `/btt/`, the way GitHub Pages serves it, for trying the site on
// this machine. Run `npm run demo` to build the viewer and start this server.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("../dist", import.meta.url)));
const base = "/btt/";
const port = Number(process.env.PORT ?? 4173);

const contentTypes = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webmanifest": "application/manifest+json",
};

const server = createServer((request, response) => {
  const path = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname);
  if (!path.startsWith(base)) {
    response.writeHead(302, { location: base });
    response.end();
    return;
  }
  const relative = normalize(path.slice(base.length)) || "index.html";
  let file = join(root, relative);
  // A path that climbs out of the folder is never a file of the site.
  if (!file.startsWith(root) || !existsSync(file) || statSync(file).isDirectory()) {
    file = join(root, "index.html");
  }
  response.writeHead(200, { "content-type": contentTypes[extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(response);
});

server.listen(port, "127.0.0.1", () => {
  console.log(`The viewer is at http://127.0.0.1:${port}${base}`);
});
