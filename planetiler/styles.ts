// The layers of the Protomaps styles, for every flavor and language of the package at the commit of
// the profile: ci/profile.sh runs it inside its styles/ folder, and writes <out>/<flavor>/<lang>.json.
// The module composes the rest of each style (its source, glyphs and sprite) when it serves it.
import { mkdirSync, writeFileSync } from "node:fs";
import { language_script_pairs, layers, namedFlavor } from "./src/index";

const out = process.argv[2];
for (const flavor of ["light", "dark", "white", "grayscale", "black"]) {
  mkdirSync(`${out}/${flavor}`, { recursive: true });
  for (const { lang } of language_script_pairs) {
    writeFileSync(`${out}/${flavor}/${lang}.json`, JSON.stringify(layers("protomaps", namedFlavor(flavor), { lang })));
  }
}
