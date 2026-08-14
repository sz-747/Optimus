import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = fileURLToPath(new URL("../", import.meta.url));
const sourceRoot = join(packageRoot, "src");
const colorLiteral = /#[0-9a-f]{3,8}\b/gi;
const offenders = [];

async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(path);
    } else if (entry.isFile() && /\.(css|js)$/.test(entry.name) && entry.name !== "tokens.css") {
      const contents = await readFile(path, "utf8");
      if (colorLiteral.test(contents)) {
        offenders.push(relative(packageRoot, path));
      }
      colorLiteral.lastIndex = 0;
    }
  }
}

await walk(sourceRoot);

if (offenders.length > 0) {
  console.error("Use the shared CSS tokens instead of literal colors:");
  for (const file of offenders) console.error(`- ${file}`);
  process.exitCode = 1;
} else {
  console.log("Frontend token check passed.");
}
