// @vitest-environment node
import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { buildTauriDevArgs, parsePort, superviseChild, type Port } from "./dev-utils.ts";

describe("parsePort", () => {
  it("reads a port followed by a newline", () => {
    expect(parsePort("4123\n")).toBe(4123);
  });

  it.each(["", "abc", "0", "70000", "12 34", "-5"])("rejects %j", (output) => {
    expect(parsePort(output)).toBeNull();
  });
});

describe("buildTauriDevArgs", () => {
  const port = 4123 as Port;

  function configOf(args: string[]): { build: { devUrl: string; beforeDevCommand: string } } {
    const index = args.indexOf("--config");
    expect(index).toBeGreaterThanOrEqual(0);
    return JSON.parse(args[index + 1] ?? "");
  }

  it("runs tauri dev", () => {
    expect(buildTauriDevArgs(port, []).slice(0, 2)).toEqual(["tauri", "dev"]);
  });

  it("puts the port in the dev URL", () => {
    expect(configOf(buildTauriDevArgs(port, [])).build.devUrl).toBe("http://localhost:4123");
  });

  it("starts Vite on the port and fails when the port is taken", () => {
    const command = configOf(buildTauriDevArgs(port, [])).build.beforeDevCommand;
    expect(command).toContain("pnpm exec vite");
    expect(command).toContain("--port 4123");
    expect(command).toContain("--strictPort");
  });

  it("gives no separator when there are no extra arguments", () => {
    expect(buildTauriDevArgs(port, [])).not.toContain("--");
  });

  it("puts extra arguments after the separator", () => {
    const args = buildTauriDevArgs(port, ["--release", "--no-watch"]);
    expect(args.slice(args.indexOf("--"))).toEqual(["--", "--release", "--no-watch"]);
  });
});

describe("superviseChild", () => {
  function setup() {
    const child = new EventEmitter();
    const exit = vi.fn();
    const fail = vi.fn();
    superviseChild(child, "cargo tauri dev", exit, fail);
    return { child, exit, fail };
  }

  it("fails with the name and the error when the child does not start", () => {
    const { child, exit, fail } = setup();
    child.emit("error", new Error("spawn cargo ENOENT"));
    expect(fail).toHaveBeenCalledWith("Failed to start cargo tauri dev: spawn cargo ENOENT");
    expect(exit).not.toHaveBeenCalled();
  });

  it("exits with code 0 when the child exits with code 0", () => {
    const { child, exit } = setup();
    child.emit("exit", 0, null);
    expect(exit).toHaveBeenCalledWith(0);
  });

  it("exits with the code of the child", () => {
    const { child, exit } = setup();
    child.emit("exit", 3, null);
    expect(exit).toHaveBeenCalledWith(3);
  });

  it("exits with code 1 when a signal stops the child", () => {
    const { child, exit } = setup();
    child.emit("exit", null, "SIGTERM");
    expect(exit).toHaveBeenCalledWith(1);
  });
});
