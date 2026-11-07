// Cambia la versión de Volt en los tres sitios donde vive y actualiza Cargo.lock.
// Uso: node scripts/bump-version.mjs 0.2.0
import { readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-[\w.]+)?$/.test(version ?? "")) {
  console.error("Uso: node scripts/bump-version.mjs <versión semver, p. ej. 0.2.0>");
  process.exit(1);
}

const json = (path, edit) => {
  const data = JSON.parse(readFileSync(path, "utf8"));
  edit(data);
  writeFileSync(path, JSON.stringify(data, null, 2) + "\n");
};

json("package.json", (d) => (d.version = version));
json("package-lock.json", (d) => {
  d.version = version;
  if (d.packages?.[""]) d.packages[""].version = version;
});
json("src-tauri/tauri.conf.json", (d) => (d.version = version));

const cargo = "src-tauri/Cargo.toml";
writeFileSync(
  cargo,
  readFileSync(cargo, "utf8").replace(/^version = ".*"$/m, `version = "${version}"`),
);
// Solo el paquete propio en Cargo.lock (no toca dependencias).
execFileSync("cargo", ["update", "-p", "volt", "--offline"], { cwd: "src-tauri", stdio: "inherit" });

console.log(`Versión ${version}. Ahora:\n  git commit -am "Volt ${version}" && git tag v${version} && git push --follow-tags`);
