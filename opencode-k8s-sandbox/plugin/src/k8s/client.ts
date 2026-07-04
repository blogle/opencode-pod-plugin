import * as k8s from "@kubernetes/client-node";

let kc: k8s.KubeConfig | null = null;
let coreV1Api: k8s.CoreV1Api | null = null;

export function initKubeConfig(): k8s.KubeConfig {
  if (!kc) {
    kc = new k8s.KubeConfig();
    // Try in-cluster first, fall back to default kubeconfig
    try {
      kc.loadFromCluster();
      const server = kc.getCurrentCluster()?.server;
      // loadFromCluster may succeed but produce an invalid URL in dev environments
      if (!server || server.includes("undefined") || !server.startsWith("https://")) {
        kc = new k8s.KubeConfig();
        kc.loadFromDefault();
      }
    } catch {
      kc = new k8s.KubeConfig();
      kc.loadFromDefault();
    }
  }
  return kc;
}

export function getCoreV1Api(): k8s.CoreV1Api {
  if (!coreV1Api) {
    const config = initKubeConfig();
    coreV1Api = config.makeApiClient(k8s.CoreV1Api);
  }
  return coreV1Api;
}

export function getExecApi(): k8s.Exec {
  const config = initKubeConfig();
  return new k8s.Exec(config);
}
