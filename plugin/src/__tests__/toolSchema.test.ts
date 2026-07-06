import { describe, it, expect } from "vitest";
import { tool } from "@opencode-ai/plugin";

/**
 * Contract test: verifies that all tool definitions produce valid
 * Zod-compatible schemas that OpenCode can parse and validate.
 */
function defineTestTools() {
  return {
    bash: tool({
      description: "Execute a shell command inside the sandbox pod",
      args: {
        command: tool.schema.string().describe("The command to execute"),
      },
      async execute(args) {
        return `executed: ${args.command}`;
      },
    }),

    read: tool({
      description: "Read a file from the sandbox pod",
      args: {
        filePath: tool.schema.string().describe("Path to the file"),
      },
      async execute(args) {
        return `read: ${args.filePath}`;
      },
    }),

    write: tool({
      description: "Write content to a file in the sandbox pod",
      args: {
        filePath: tool.schema.string().describe("Path to the file"),
        content: tool.schema.string().describe("Content to write"),
      },
      async execute(args) {
        return `wrote: ${args.filePath}`;
      },
    }),

    edit: tool({
      description: "Edit a file using string replacement",
      args: {
        filePath: tool.schema.string().describe("Path to the file"),
        oldString: tool.schema.string().describe("String to replace"),
        newString: tool.schema.string().describe("Replacement string"),
      },
      async execute(args) {
        return `edited: ${args.filePath}`;
      },
    }),

    list: tool({
      description: "List files in a directory",
      args: {
        path: tool.schema.string().describe("Directory path"),
      },
      async execute(args) {
        return `listed: ${args.path}`;
      },
    }),

    glob: tool({
      description: "Find files by glob pattern",
      args: {
        pattern: tool.schema.string().describe("Glob pattern"),
      },
      async execute(args) {
        return `glob: ${args.pattern}`;
      },
    }),

    grep: tool({
      description: "Search file contents using regex",
      args: {
        pattern: tool.schema.string().describe("Regex pattern"),
        path: tool.schema.string().describe("Directory to search in"),
        include: tool.schema.string().optional().describe("File pattern to include"),
      },
      async execute(args) {
        return `grep: ${args.pattern} in ${args.path}`;
      },
    }),

    "preview-link": tool({
      description: "Get a URL to access a service in the sandbox",
      args: {
        port: tool.schema.number().describe("Port number"),
      },
      async execute(args) {
        return `http://localhost:${args.port}`;
      },
    }),

    multiedit: tool({
      description: "Apply multiple edits",
      args: {
        operations: tool.schema
          .array(
            tool.schema.object({
              path: tool.schema.string(),
              oldString: tool.schema.string(),
              newString: tool.schema.string(),
            })
          )
          .describe("Edit operations"),
      },
      async execute(args) {
        return `multiedit: ${args.operations.length} ops`;
      },
    }),

    apply_patch: tool({
      description: "Apply a patch in OpenCode marker format",
      args: {
        patchText: tool.schema.string().describe("Patch content"),
      },
      async execute(args) {
        return `patched`;
      },
    }),
  };
}

describe("Tool schema contract", () => {
  const tools = defineTestTools();

  const toolNames = [
    "bash",
    "read",
    "write",
    "edit",
    "list",
    "glob",
    "grep",
    "preview-link",
    "multiedit",
    "apply_patch",
  ];

  for (const name of toolNames) {
    it(`${name}: should be a valid tool object with description and args`, () => {
      const t = tools[name as keyof typeof tools];
      expect(t).toBeDefined();
      expect(typeof t.description).toBe("string");
      expect(t.description.length).toBeGreaterThan(0);
      expect(t.args).toBeDefined();
      expect(typeof t.execute).toBe("function");
    });

    it(`${name}: args should be Zod schemas with parse capability`, () => {
      const t = tools[name as keyof typeof tools];
      for (const [key, schema] of Object.entries(t.args)) {
        expect(typeof schema).toBe("object");
        // Zod schemas have .parse, .safeParse
        expect(typeof (schema as any).parse).toBe("function");
        expect(typeof (schema as any).safeParse).toBe("function");
      }
    });
  }

  it("bash: should validate correct args", () => {
    const t = tools.bash;
    const result = t.args.command.safeParse("ls -la");
    expect(result.success).toBe(true);
  });

  it("bash: should reject non-string command", () => {
    const t = tools.bash;
    const result = (t.args.command as any).safeParse(123);
    expect(result.success).toBe(false);
  });

  it("write: should validate correct args", () => {
    const t = tools.write;
    const filePath = t.args.filePath.safeParse("src/index.ts");
    const content = t.args.content.safeParse("hello world");
    expect(filePath.success).toBe(true);
    expect(content.success).toBe(true);
  });

  it("multiedit: should validate array of operations", () => {
    const t = tools.multiedit;
    const result = t.args.operations.safeParse([
      { path: "a.ts", oldString: "foo", newString: "bar" },
    ]);
    expect(result.success).toBe(true);
  });

  it("grep: should handle optional include arg", () => {
    const t = tools.grep;
    const without = t.args.pattern.safeParse("TODO");
    const withPath = t.args.path.safeParse("src");
    const withInclude = t.args.include?.safeParse("*.ts");
    expect(without.success).toBe(true);
    expect(withPath.success).toBe(true);
    // include is optional, so undefined should be fine
    expect(withInclude?.success).toBe(true);
  });

  it("preview-link: should validate port as number", () => {
    const t = tools["preview-link"];
    const result = t.args.port.safeParse(3000);
    expect(result.success).toBe(true);
    const bad = t.args.port.safeParse("3000");
    expect(bad.success).toBe(false);
  });
});
