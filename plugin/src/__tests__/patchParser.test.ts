import { describe, it, expect } from "vitest";
import { parsePatch, applyHunks } from "../tools/patchParser.js";

describe("parsePatch", () => {
  it("should parse an Add File hunk", () => {
    const patch = `*** Begin Patch
*** Add File: src/hello.ts
+export const hello = "world";
+console.log(hello);
*** End Patch`;

    const { hunks } = parsePatch(patch);
    expect(hunks).toHaveLength(1);
    expect(hunks[0]).toEqual({
      type: "add",
      path: "src/hello.ts",
      contents: 'export const hello = "world";\nconsole.log(hello);',
    });
  });

  it("should parse a Delete File hunk", () => {
    const patch = `*** Begin Patch
*** Delete File: src/old.ts
*** End Patch`;

    const { hunks } = parsePatch(patch);
    expect(hunks).toHaveLength(1);
    expect(hunks[0]).toEqual({ type: "delete", path: "src/old.ts" });
  });

  it("should parse an Update File hunk", () => {
    const patch = `*** Begin Patch
*** Update File: src/index.ts
@@ old
-old line
+new line
*** End Patch`;

    const { hunks } = parsePatch(patch);
    expect(hunks).toHaveLength(1);
    expect(hunks[0].type).toBe("update");
    expect(hunks[0].path).toBe("src/index.ts");
    expect(hunks[0].oldLines).toEqual(["old line"]);
    expect(hunks[0].newLines).toEqual(["new line"]);
  });

  it("should parse a Move to within an Update", () => {
    const patch = `*** Begin Patch
*** Update File: src/old-name.ts
*** Move to: src/new-name.ts
-old content
+new content
*** End Patch`;

    const { hunks } = parsePatch(patch);
    expect(hunks).toHaveLength(1);
    expect(hunks[0].movePath).toBe("src/new-name.ts");
  });

  it("should parse multiple hunks", () => {
    const patch = `*** Begin Patch
*** Add File: src/new.ts
+new file content

*** Update File: src/existing.ts
-old
+new

*** Delete File: src/old.ts

*** End Patch`;

    const { hunks } = parsePatch(patch);
    expect(hunks).toHaveLength(3);
    expect(hunks[0].type).toBe("add");
    expect(hunks[1].type).toBe("update");
    expect(hunks[2].type).toBe("delete");
  });

  it("should throw on missing Begin Patch marker", () => {
    expect(() => parsePatch("no markers here")).toThrow("Missing *** Begin Patch");
  });
});

describe("applyHunks", () => {
  it("should apply an add hunk to empty content", () => {
    const result = applyHunks("", [
      { type: "add", path: "test.ts", contents: "hello world" },
    ]);
    expect(result).toBe("hello world\n");
  });

  it("should apply a delete hunk", () => {
    const result = applyHunks("some content\n", [
      { type: "delete", path: "test.ts" },
    ]);
    expect(result).toBe("");
  });

  it("should apply an update hunk with exact match", () => {
    const result = applyHunks("line1\nline2\nline3\n", [
      {
        type: "update",
        path: "test.ts",
        oldLines: ["line2"],
        newLines: ["line2-updated"],
      },
    ]);
    expect(result).toBe("line1\nline2-updated\nline3\n");
  });

  it("should apply an update hunk with trailing whitespace tolerance", () => {
    const result = applyHunks("line1\nline2  \nline3\n", [
      {
        type: "update",
        path: "test.ts",
        oldLines: ["line2"],
        newLines: ["line2-updated"],
      },
    ]);
    expect(result).toBe("line1\nline2-updated\nline3\n");
  });

  it("should apply an update hunk with multi-line replacement", () => {
    const result = applyHunks("a\nb\nc\nd\n", [
      {
        type: "update",
        path: "test.ts",
        oldLines: ["b", "c"],
        newLines: ["B", "C"],
      },
    ]);
    expect(result).toBe("a\nB\nC\nd\n");
  });

  it("should throw when old text not found", () => {
    expect(() =>
      applyHunks("line1\nline2\n", [
        {
          type: "update",
          path: "test.ts",
          oldLines: ["nonexistent"],
          newLines: ["replaced"],
        },
      ])
    ).toThrow("Could not find old text");
  });
});
