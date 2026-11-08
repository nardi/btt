// Builds the backend as WebAssembly and writes the module and its JavaScript glue to
// `src/wasm`, where the worker imports them. Needs the `wasm32-unknown-unknown` target and a
// `wasm-bindgen` command-line tool of the same version as the `wasm-bindgen` crate.

import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../..", import.meta.url));
const outputDirectory = fileURLToPath(new URL("../src/wasm", import.meta.url));
const profile = process.argv.includes("--debug") ? "debug" : "release";

const cargoArguments = ["build", "--target", "wasm32-unknown-unknown", "-p", "btt-web"];
if (profile === "release") {
  cargoArguments.push("--release");
}
execFileSync("cargo", cargoArguments, { cwd: root, stdio: "inherit" });

execFileSync(
  "wasm-bindgen",
  [
    `target/wasm32-unknown-unknown/${profile}/btt_web.wasm`,
    "--target",
    "web",
    "--out-dir",
    outputDirectory,
    "--out-name",
    "btt_web",
  ],
  { cwd: root, stdio: "inherit" },
);

// Release modules go through wasm-opt from binaryen, which shrinks the code. It rewrites the file
// in place. Older binaryen releases move the `__wbindgen_externrefs` table, which breaks the page,
// so the version is pinned in `package.json`.
if (profile === "release") {
  const modulePath = join(outputDirectory, "btt_web_bg.wasm");
  const wasmOpt = join(root, "node_modules", ".bin", "wasm-opt");
  execFileSync(wasmOpt, ["-Oz", "--strip-debug", "--strip-producers", "-o", modulePath, modulePath], {
    cwd: root,
    stdio: "inherit",
  });
}
