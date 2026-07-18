import type { Plugin } from "@opencode-ai/plugin";
import { readFile } from "node:fs/promises";
import { kubernetesAdapter } from "./adapter.js";
import { GatewayClient } from "./client.js";

export const OPENCODE_VERSION = "1.18.3";
export const OPENCODE_COMMIT = "127bdb30784d508cc556c71a0f32b508a3061517";

interface Options {
  gatewayUrl?: unknown;
  gatewayToken?: unknown;
  gatewayTokenFile?: unknown;
}

export const KubernetesSandboxPlugin: Plugin = async (
  { experimental_workspace },
  rawOptions = {},
) => {
  const options = rawOptions as Options;
  if (typeof options.gatewayUrl !== "string" || !options.gatewayUrl) {
    throw new Error("Kubernetes Sandbox plugin requires gatewayUrl");
  }
  if (options.gatewayToken !== undefined && typeof options.gatewayToken !== "string") {
    throw new Error("Kubernetes Sandbox plugin gatewayToken must be a string");
  }
  if (options.gatewayTokenFile !== undefined && typeof options.gatewayTokenFile !== "string") {
    throw new Error("Kubernetes Sandbox plugin gatewayTokenFile must be a string");
  }
  if (options.gatewayToken && options.gatewayTokenFile) {
    throw new Error("Kubernetes Sandbox plugin accepts gatewayToken or gatewayTokenFile, not both");
  }
  const gatewayToken = options.gatewayTokenFile
    ? (await readFile(options.gatewayTokenFile, "utf8")).trim()
    : options.gatewayToken;
  if (options.gatewayTokenFile && !gatewayToken) {
    throw new Error("Kubernetes Sandbox plugin gateway token file is empty");
  }
  experimental_workspace.register(
    "kubernetes",
    kubernetesAdapter(new GatewayClient(options.gatewayUrl, fetch, gatewayToken as string | undefined)),
  );
  return {};
};

export default KubernetesSandboxPlugin;
