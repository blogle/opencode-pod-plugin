import { existsSync } from "fs";
import { join } from "path";
import { Config } from "./config.js";
import { unwrapError } from "./k8s/exec.js";

export interface RepoMapping {
  name: string;
  url: string;
  localPath: string;
}

/**
 * Shell command executor interface.
 * Compatible with Bun's $ shell API.
 */
interface Shell {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<string>;
}

/**
 * Clone or update repos from the manifest into subdirectories.
 * Returns a mapping of local directory paths to repo URLs.
 *
 * Uses Bun shell ($) for git operations. On init, existing repos
 * are re-pulled (fetch + reset) to pick up upstream changes.
 * New repos are shallow cloned.
 */
export async function cloneRepos(
  config: Config,
  worktree: string,
  $: Shell
): Promise<Map<string, RepoMapping>> {
  const repoBase = join(worktree, config.repoBaseDir);
  const mapping = new Map<string, RepoMapping>();

  for (const [name, url] of Object.entries(config.repos)) {
    const targetDir = join(repoBase, name);

    if (existsSync(targetDir)) {
      // Re-pull existing repo
      console.log(`[repos] Updating ${name} at ${targetDir}`);
      try {
        await $`git -C ${targetDir} fetch --depth=1 origin`;
        await $`git -C ${targetDir} reset --hard origin/HEAD`;
      } catch (error) {
        console.warn(`[repos] Failed to update ${name}: ${unwrapError(error)}`);
        // Continue anyway — stale repo is better than no repo
      }
    } else {
      // Shallow clone
      console.log(`[repos] Cloning ${name} from ${url}`);
      try {
        await $`git clone --depth=1 ${url} ${targetDir}`;
      } catch (error) {
        console.error(`[repos] Failed to clone ${name}: ${unwrapError(error)}`);
        throw new Error(`Failed to clone repository "${name}" from ${url}: ${unwrapError(error)}`);
      }
    }

    mapping.set(targetDir, { name, url, localPath: targetDir });
  }

  return mapping;
}

/**
 * Find which repo a directory belongs to.
 * Matches against the local paths in the repo mapping.
 */
export function findRepoForDirectory(
  directory: string,
  repoMapping: Map<string, RepoMapping>
): RepoMapping | undefined {
  for (const [localPath, repo] of repoMapping) {
    if (directory === localPath || directory.startsWith(localPath + "/")) {
      return repo;
    }
  }
  return undefined;
}
