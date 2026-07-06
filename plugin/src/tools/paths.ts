const WORKSPACE = "/workspace";

/**
 * Normalize a file path for use inside the sandbox container.
 * Absolute paths are passed through. Relative paths are resolved
 * against /workspace.
 */
export function sandboxPath(input: string): string {
  if (input.startsWith("/")) return input;
  return `${WORKSPACE}/${input}`;
}
