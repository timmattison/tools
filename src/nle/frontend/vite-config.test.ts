/** @vitest-environment node */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadEnv } from "vite";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import config from "./vite.config";

// The Tauri 2 CLI reads the updater signing key and its password from these
// two variables.
const SIGNING_KEY = "TAURI_SIGNING_PRIVATE_KEY";
const SIGNING_KEY_PASSWORD = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD";
// The Tauri 2 CLI exports its build variables with the prefix TAURI_ENV_.
const PLATFORM = "TAURI_ENV_PLATFORM";

const PLANTED: Readonly<Record<string, string>> = {
  [SIGNING_KEY]: "planted-signing-key",
  [SIGNING_KEY_PASSWORD]: "planted-signing-key-password",
  [PLATFORM]: "darwin",
};

// Vite gives the frontend, through import.meta.env, the variables that
// loadEnv returns for the envPrefix of the config.
function exposedToFrontend(envDir: string): Record<string, string> {
  return loadEnv("production", envDir, config.envPrefix);
}

describe("the envPrefix of the Vite config", () => {
  let envDir = "";
  const saved = new Map<string, string | undefined>();

  beforeEach(() => {
    envDir = mkdtempSync(join(tmpdir(), "nle-vite-env-"));
    for (const [name, value] of Object.entries(PLANTED)) {
      saved.set(name, process.env[name]);
      process.env[name] = value;
    }
  });

  afterEach(() => {
    for (const [name, value] of saved) {
      if (value === undefined) {
        Reflect.deleteProperty(process.env, name);
      } else {
        process.env[name] = value;
      }
    }
    saved.clear();
    rmSync(envDir, { recursive: true, force: true });
  });

  it("keeps the updater signing key and its password out of the frontend", () => {
    const exposed = exposedToFrontend(envDir);

    expect(Object.keys(exposed)).not.toContain(SIGNING_KEY);
    expect(Object.keys(exposed)).not.toContain(SIGNING_KEY_PASSWORD);
  });

  it("gives the frontend the build variables of the Tauri CLI", () => {
    const exposed = exposedToFrontend(envDir);

    expect(exposed[PLATFORM]).toBe(PLANTED[PLATFORM]);
  });
});
