import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDir, "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");
const json = (relativePath) => JSON.parse(read(relativePath));
const fail = (message) => {
  throw new Error(message);
};

const tag = process.argv[2] ?? process.env.RELEASE_TAG;
if (!tag || !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
  fail(`Release tag must look like v1.2.3, received ${JSON.stringify(tag)}`);
}

const expected = tag.slice(1);
const versions = new Map([
  ["desktop/src-tauri/tauri.conf.json", json("desktop/src-tauri/tauri.conf.json").version],
  ["desktop/package.json", json("desktop/package.json").version],
  ["desktop/package-lock.json", json("desktop/package-lock.json").version],
  ["desktop/package-lock.json root package", json("desktop/package-lock.json").packages?.[""]?.version],
]);

for (const cargoPath of ["launcher/Cargo.toml", "desktop/src-tauri/Cargo.toml"]) {
  const match = read(cargoPath).match(/\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  versions.set(cargoPath, match?.[1]);
}

const scoop = json("packaging/scoop/velgrinor.json");
versions.set("packaging/scoop/velgrinor.json", scoop.version);

for (const [file, version] of versions) {
  if (version !== expected) {
    fail(`${file} has version ${JSON.stringify(version)}, expected ${expected}`);
  }
}

const config = json("desktop/src-tauri/tauri.conf.json");
if (!config.bundle?.createUpdaterArtifacts) {
  fail("Tauri updater artifacts are disabled");
}
if (!config.plugins?.updater?.pubkey || /PLACEHOLDER/i.test(config.plugins.updater.pubkey)) {
  fail("Tauri updater public key is missing or still a placeholder");
}

const versionedTemplates = [
  ["packaging/aur/velgrinor/PKGBUILD", /^pkgver=([^\s]+)$/m],
  ["packaging/aur/velgrinor-launcher-bin/PKGBUILD", /^pkgver=([^\s]+)$/m],
  ["packaging/winget/Th0rgal.VelGrinorLauncher.yaml", /^PackageVersion:\s*([^\s]+)$/m],
  ["packaging/flathub/md.thomas.velgrinor.launcher.yml", /^\s*tag:\s*v([^\s]+)$/m],
  ["packaging/flathub/md.thomas.velgrinor.launcher.metainfo.xml", /<releases>\s*<release version="([^"]+)"/],
];
for (const [file, pattern] of versionedTemplates) {
  const version = read(file).match(pattern)?.[1];
  if (version !== expected) {
    fail(`${file} has release version ${JSON.stringify(version)}, expected ${expected}`);
  }
}

if (!scoop.url.includes(`/download/${tag}/`)) {
  fail(`packaging/scoop/velgrinor.json URL does not reference ${tag}`);
}

const winget = read("packaging/winget/Th0rgal.VelGrinorLauncher.yaml");
if (!winget.includes(`/download/${tag}/`)) {
  fail(`Winget installer URL does not reference ${tag}`);
}

console.log(`Release metadata is consistent for ${tag}`);
