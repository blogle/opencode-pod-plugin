import { describe, it, expect } from "vitest";
import { sandboxPath } from "../tools/paths.js";

describe("sandboxPath", () => {
  it("should pass through absolute paths", () => {
    expect(sandboxPath("/workspace/src/index.ts")).toBe("/workspace/src/index.ts");
    expect(sandboxPath("/etc/passwd")).toBe("/etc/passwd");
    expect(sandboxPath("/tmp/test")).toBe("/tmp/test");
  });

  it("should prefix relative paths with /workspace", () => {
    expect(sandboxPath("src/index.ts")).toBe("/workspace/src/index.ts");
    expect(sandboxPath("package.json")).toBe("/workspace/package.json");
    expect(sandboxPath(".")).toBe("/workspace/.");
  });

  it("should handle empty string", () => {
    expect(sandboxPath("")).toBe("/workspace/");
  });

  it("should handle paths with dots", () => {
    expect(sandboxPath("./src/file.ts")).toBe("/workspace/./src/file.ts");
    expect(sandboxPath("../other/file.ts")).toBe("/workspace/../other/file.ts");
  });
});
