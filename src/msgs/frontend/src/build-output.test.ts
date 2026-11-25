// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import config from "../vite.config";

const frontendDir = fileURLToPath(new URL("..", import.meta.url));
const packageDir = resolve(frontendDir, "..");
const repositoryTarget = resolve(packageDir, "../../target");

describe("the build output of the frontend", () => {
  // The repository guards (cdva's line test among them) walk the tree with the
  // ignore files turned off, and they skip only target and node_modules. A
  // minified bundle anywhere else fails them after each build.
  it("goes under the target directory of the repository", () => {
    const outDir = resolve(frontendDir, config.build?.outDir ?? "dist");
    expect(outDir.startsWith(`${repositoryTarget}/`)).toBe(true);
  });

  it("is the directory that Tauri loads", () => {
    const outDir = resolve(frontendDir, config.build?.outDir ?? "dist");
    const tauriConfig: unknown = JSON.parse(
      readFileSync(resolve(packageDir, "tauri.conf.json"), "utf8"),
    );
    expect(tauriConfig).toMatchObject({ build: { frontendDist: expect.any(String) } });
    const frontendDist = (tauriConfig as { build: { frontendDist: string } }).build.frontendDist;
    expect(resolve(packageDir, frontendDist)).toBe(outDir);
  });
});
