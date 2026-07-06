export interface PatchHunk {
  type: "add" | "delete" | "update";
  path: string;
  movePath?: string;
  contents?: string;
  oldLines?: string[];
  newLines?: string[];
}

export interface ParsedPatch {
  hunks: PatchHunk[];
}

function stripTrailingNewline(s: string): string {
  return s.endsWith("\n") ? s.slice(0, -1) : s;
}

/**
 * Parse OpenCode's marker-line patch format into structured hunks.
 *
 * Format:
 *   *** Begin Patch
 *   *** Add File: path/to/file
 *   +line content
 *
 *   *** Delete File: path/to/file
 *
 *   *** Update File: path/to/file
 *   @@ context line
 *   -old line
 *   +new line
 *   unchanged line
 *
 *   *** Move to: new/path
 *
 *   *** End Patch
 */
export function parsePatch(patchText: string): ParsedPatch {
  const lines = patchText.split("\n");
  const hunks: PatchHunk[] = [];

  let i = 0;

  // Skip to *** Begin Patch
  while (i < lines.length && lines[i].trim() !== "*** Begin Patch") {
    i++;
  }
  if (i >= lines.length) {
    throw new Error("Missing *** Begin Patch marker");
  }
  i++; // skip the Begin marker

  while (i < lines.length) {
    const line = lines[i];

    if (line.trim() === "*** End Patch" || line.trim() === "") {
      if (line.trim() === "*** End Patch") break;
      i++;
      continue;
    }

    // *** Add File: <path>
    const addMatch = line.match(/^\*\*\* Add File:\s*(.+)$/);
    if (addMatch) {
      const path = addMatch[1].trim();
      i++;
      const contentLines: string[] = [];
      while (i < lines.length && !lines[i].startsWith("***")) {
        // Lines starting with + are content (strip the + prefix)
        if (lines[i].startsWith("+")) {
          contentLines.push(lines[i].slice(1));
        } else if (lines[i].trim() === "") {
          // blank line in patch = blank line in file
          contentLines.push("");
        }
        i++;
      }
      hunks.push({
        type: "add",
        path,
        contents: contentLines.join("\n"),
      });
      continue;
    }

    // *** Delete File: <path>
    const deleteMatch = line.match(/^\*\*\* Delete File:\s*(.+)$/);
    if (deleteMatch) {
      const path = deleteMatch[1].trim();
      hunks.push({ type: "delete", path });
      i++;
      continue;
    }

    // *** Update File: <path>
    const updateMatch = line.match(/^\*\*\* Update File:\s*(.+)$/);
    if (updateMatch) {
      const path = updateMatch[1].trim();
      i++;

      const oldLines: string[] = [];
      const newLines: string[] = [];
      let movePath: string | undefined;

      while (i < lines.length) {
        const ul = lines[i];

        // Exit the inner loop on any *** marker EXCEPT Move to
        if (ul.startsWith("***")) {
          const moveCheck = ul.match(/^\*\*\* Move to:\s*(.+)$/);
          if (moveCheck) {
            movePath = moveCheck[1].trim();
            i++;
            continue;
          }
          break;
        }

        if (ul.startsWith("-")) {
          oldLines.push(stripTrailingNewline(ul.slice(1)));
        } else if (ul.startsWith("+")) {
          newLines.push(stripTrailingNewline(ul.slice(1)));
        } else if (ul.startsWith("@@")) {
          // Context line marker — skip
        } else if (ul.trim() === "") {
          // blank line
        } else {
          // Unchanged context line — include in both old and new
          oldLines.push(stripTrailingNewline(ul));
          newLines.push(stripTrailingNewline(ul));
        }
        i++;
      }

      hunks.push({
        type: "update",
        path,
        movePath,
        oldLines,
        newLines,
      });
      continue;
    }

    i++;
  }

  return { hunks };
}

/**
 * Fuzzy-match oldLines against file content using OpenCode's 4-level strategy:
 * 1. Exact match
 * 2. Match after trimming trailing whitespace per line
 * 3. Match after trimming leading+trailing whitespace per line
 * 4. Match after Unicode normalization (smart quotes, dashes)
 */
function normalizeUnicode(s: string): string {
  return s
    .replace(/[\u2018\u2019\u201A\uFFFD]/g, "'")
    .replace(/[\u201C\u201D\u201E\uFFFD]/g, '"')
    .replace(/[\u2013\u2014]/g, "-")
    .replace(/\u2026/g, "...");
}

function normalizeLines(
  lines: string[],
  mode: "exact" | "trimTrail" | "trimAll" | "unicode"
): string[] {
  switch (mode) {
    case "exact":
      return lines;
    case "trimTrail":
      return lines.map((l) => l.trimEnd());
    case "trimAll":
      return lines.map((l) => l.trim());
    case "unicode":
      return lines.map((l) => normalizeUnicode(l.trim()));
  }
}

/**
 * Apply parsed hunks to a file's content, returning the updated content.
 * For update hunks, uses fuzzy matching to find the old text.
 */
export function applyHunks(
  fileContent: string,
  hunks: PatchHunk[]
): string {
  let content = fileContent;

  for (const hunk of hunks) {
    if (hunk.type === "add") {
      // For add, if content already exists we overwrite; otherwise append
      if (content.length > 0 && !content.endsWith("\n")) {
        content += "\n";
      }
      content += hunk.contents ?? "";
      if (!content.endsWith("\n")) {
        content += "\n";
      }
    } else if (hunk.type === "delete") {
      content = "";
    } else if (hunk.type === "update") {
      if (!hunk.oldLines || !hunk.newLines) continue;

      const oldText = hunk.oldLines.join("\n");
      const newText = hunk.newLines.join("\n");

      // Try 4-level fuzzy matching
      const modes: Array<"exact" | "trimTrail" | "trimAll" | "unicode"> = [
        "exact",
        "trimTrail",
        "trimAll",
        "unicode",
      ];

      let found = false;
      for (const mode of modes) {
        const normalizedOld = normalizeLines(hunk.oldLines, mode).join("\n");
        const fileLines = content.split("\n");
        const normalizedFile = normalizeLines(fileLines, mode).join("\n");

        const idx = normalizedFile.indexOf(normalizedOld);
        if (idx !== -1) {
          // Find the actual (un-normalized) position by counting lines
          const prefix = normalizeLines(fileLines.slice(0, fileLines.length), mode)
            .join("\n")
            .slice(0, idx);
          const matchLineCount = prefix.split("\n").length - 1;

          // Replace in original content
          const fileLinesActual = content.split("\n");
          fileLinesActual.splice(
            matchLineCount,
            hunk.oldLines.length,
            ...hunk.newLines
          );
          content = fileLinesActual.join("\n");
          found = true;
          break;
        }
      }

      if (!found) {
        throw new Error(
          `Could not find old text in file for update hunk. Expected:\n${hunk.oldLines.join("\n")}`
        );
      }
    }
  }

  return content;
}
