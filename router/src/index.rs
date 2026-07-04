use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use k8s_openapi::api::core::v1::Pod;
use kube::api::Api;
use kube::runtime::{reflector, watcher};
use kube::Client;

use crate::health::SharedMetrics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodInfo {
    pub pod_ip: String,
    pub namespace: String,
    pub name: String,
}

pub type SandboxIndex = Arc<RwLock<HashMap<String, PodInfo>>>;

pub fn new_index() -> SandboxIndex {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Spawns a reflector that watches pods with the given label key and keeps the index updated.
pub async fn start_reflector(
    client: Client,
    namespace: Option<&str>,
    label_key: &str,
    index: SandboxIndex,
    metrics: SharedMetrics,
) -> Result<(), kube::Error> {
    let pods: Api<Pod> = match namespace {
        Some(ns) => Api::namespaced(client, ns),
        None => Api::all(client),
    };

    // Kubernetes "not empty" selector: key!= (empty value means "exists and is not empty")
    let label_selector = format!("{}!=", label_key);

    // Create writer and reader for the reflector store
    let writer = reflector::store::Writer::<Pod>::default();
    let reader = writer.as_reader();

    // Create the watcher config with label selector
    let wc = watcher::Config {
        label_selector: Some(label_selector),
        ..Default::default()
    };

    let w = watcher(pods, wc);
    let rf = reflector(writer, w);

    // Spawn the reflector watcher in the background using Box::pin for the stream
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut r = Box::pin(rf);
        while let Some(event) = r.next().await {
            match event {
                Ok(_event) => {
                    tracing::debug!("reflector event");
                    metrics_clone.reflector_healthy.set(1);
                }
                Err(e) => {
                    tracing::error!("reflector error: {}", e);
                    metrics_clone.reflector_healthy.set(0);
                }
            }
        }
        tracing::warn!("reflector stream ended");
        metrics_clone.reflector_healthy.set(0);
    });

    // Spawn the index updater that reads from the reflector store
    let label_key_owned = label_key.to_string();
    tokio::spawn(async move {
        // Give the reflector a moment to populate
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        loop {
            {
                let pods = reader.state();
                let mut idx = index.write().await;
                let mut seen = std::collections::HashSet::new();

                for pod in pods.iter() {
                    // Only include pods that are Running with a pod IP
                    if let Some(ref status) = pod.status {
                        if status.phase.as_deref() == Some("Running") {
                            if let Some(ref pod_ip) = status.pod_ip {
                                let meta = &pod.metadata;
                                if let Some(ref labels) = meta.labels {
                                    if let Some(sandbox_id) = labels.get(&label_key_owned) {
                                        if let Some(ref ns) = meta.namespace {
                                            let name = meta.name.clone().unwrap_or_default();
                                            let prev = idx.insert(
                                                sandbox_id.clone(),
                                                PodInfo {
                                                    pod_ip: pod_ip.clone(),
                                                    namespace: ns.clone(),
                                                    name: name.clone(),
                                                },
                                            );
                                            if prev.as_ref().is_none_or(|p| {
                                                p.pod_ip != *pod_ip || p.name != name
                                            }) {
                                                tracing::info!(
                                                    "index update: sandbox={} pod={}/{} ip={}",
                                                    sandbox_id,
                                                    ns,
                                                    name,
                                                    pod_ip
                                                );
                                            }
                                            seen.insert(sandbox_id.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Remove stale entries
                let keys: Vec<String> = idx.keys().cloned().collect();
                for key in keys {
                    if !seen.contains(&key) {
                        tracing::info!("index remove: sandbox={}", key);
                        idx.remove(&key);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });

    Ok(())
}
