// Checks that a release tag matches every version stamped into the build.
// Usage: node tools/check-release-version.mjs v1.2.3
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const json = (path) => JSON.parse(read(path));

function tomlVersion(path, section) {
  let inSection = false;
  for (const line of read(path).split(/\r?\n/)) {
    const header = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (header) inSection = header[1].trim() === section;
    const version = inSection && line.match(/^\s*version\s*=\s*"([^"]+)"/);
    if (version) return version[1];
  }
  return undefined;
}

const tag = process.argv[2] ?? "";
const match = tag.match(/^v(\d+\.\d+\.\d+)$/);
if (!match) {
  console.error(`Release tag must look like v1.2.3, got "${tag}".`);
  process.exit(1);
}
const expected = match[1];
const lock = json("package-lock.json");
const versions = {
  "package.json": json("package.json").version,
  "package-lock.json": lock.version,
  'package-lock.json packages[""]': lock.packages?.[""]?.version,
  "Cargo.toml [workspace.package]": tomlVersion("Cargo.toml", "workspace.package"),
  "src-tauri/Cargo.toml [package]": tomlVersion("src-tauri/Cargo.toml", "package"),
  "src-tauri/tauri.conf.json": json("src-tauri/tauri.conf.json").version,
};
const mismatches = Object.entries(versions).filter(([, version]) => version !== expected);
for (const [source, version] of mismatches) {
  console.error(`${source} has version ${version ?? "(missing)"}, but the tag is ${tag}.`);
}
if (mismatches.length > 0) process.exit(1);
console.log(`All ${Object.keys(versions).length} version fields match ${tag}.`);
