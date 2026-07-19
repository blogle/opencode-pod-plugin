use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use k8s_openapi::{
    api::core::v1::{Pod, Secret, Service},
    ByteString,
};
use kube::{
    api::{DeleteParams, ListParams, Patch, PatchParams, PostParams},
    runtime::watcher,
    Api, Client, ResourceExt,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{
    config::{Config, Project, ProjectEnvironment},
    state::Workspace,
};

const MANAGER: &str = "opencode-gateway";
pub const OPENCODE_PORT: u16 = 4096;
pub const SUPERVISOR_PORT: u16 = 4097;
pub const CHECKPOINT_PORT: u16 = 4098;

#[derive(Clone, Debug)]
pub struct Provisioned {
    pub image_digest: String,
}

#[async_trait]
pub trait Orchestrator: Send + Sync {
    async fn provision(
        &self,
        workspace: &Workspace,
        project: &Project,
        has_profile: bool,
    ) -> Result<Provisioned>;
    async fn suspend(&self, workspace: &Workspace) -> Result<()>;
    async fn delete(&self, workspace: &Workspace) -> Result<()>;
    async fn put_env_profile(&self, owner: &str, project: &str, content: &[u8]) -> Result<()>;
    async fn delete_env_profile(&self, owner: &str, project: &str) -> Result<()>;
    async fn ready_pod_ip(&self, preview_key: &str) -> Option<String>;
    async fn health(&self) -> Result<()>;
}

pub struct Kubernetes {
    client: Client,
    config: Arc<Config>,
    pod_ips: Arc<RwLock<HashMap<String, String>>>,
}

impl Kubernetes {
    pub async fn new(client: Client, config: Arc<Config>) -> Result<Arc<Self>> {
        let this = Arc::new(Self {
            client,
            config,
            pod_ips: Arc::new(RwLock::new(HashMap::new())),
        });
        this.start_pod_watch();
        Ok(this)
    }

    fn start_pod_watch(self: &Arc<Self>) {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let index = self.pod_ips.clone();
        tokio::spawn(async move {
            let stream = watcher(
                pods,
                watcher::Config::default().labels("opencode.dev/managed-by=gateway"),
            );
            futures::pin_mut!(stream);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(watcher::Event::Apply(pod) | watcher::Event::InitApply(pod)) => {
                        update_index(&index, &pod).await
                    }
                    Ok(watcher::Event::Delete(pod)) => remove_index(&index, &pod).await,
                    Ok(watcher::Event::Init) => index.write().await.clear(),
                    Ok(watcher::Event::InitDone) => {}
                    Err(error) => {
                        tracing::error!(operation="pod-watch", %error, "Kubernetes Pod watch failed")
                    }
                }
            }
            tracing::error!(operation = "pod-watch", "Kubernetes Pod watch ended");
        });
    }

    async fn apply<T>(&self, api: &Api<T>, name: &str, value: serde_json::Value) -> Result<()>
    where
        T: Clone
            + serde::de::DeserializeOwned
            + serde::Serialize
            + std::fmt::Debug
            + kube::Resource<DynamicType = ()>,
    {
        api.patch(
            name,
            &PatchParams::apply(MANAGER).force(),
            &Patch::Apply(value),
        )
        .await?;
        Ok(())
    }

    async fn reconcile_stable(&self, workspace: &Workspace) -> Result<()> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.config.namespace);
        self.apply(
            &services,
            &workspace.service_name,
            service_manifest(&self.config.namespace, workspace),
        )
        .await?;
        self.apply(
            &secrets,
            &runtime_secret_name(workspace),
            runtime_secret_manifest(&self.config.namespace, workspace)?,
        )
        .await?;
        Ok(())
    }

    async fn checkpoint_before_delete(&self, workspace: &Workspace, pod: &Pod) -> Result<()> {
        let ip = pod
            .status
            .as_ref()
            .and_then(|status| status.pod_ip.as_deref())
            .context("sandbox Pod has no IP for final checkpoint")?;
        let host = if ip.contains(':') {
            format!("[{ip}]")
        } else {
            ip.to_owned()
        };
        request_supervisor_checkpoint(
            &format!("http://{host}:{SUPERVISOR_PORT}/checkpoint"),
            &workspace.runtime_token,
            Duration::from_secs(self.config.lifecycle.ready_timeout_seconds),
        )
        .await
    }

    async fn wait_ready(&self, workspace: &Workspace) -> Result<Provisioned> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(self.config.lifecycle.ready_timeout_seconds);
        loop {
            if tokio::time::Instant::now() >= deadline {
                bail!("sandbox Pod did not become ready before timeout");
            }
            if let Some(pod) = pods.get_opt(&pod_name(workspace)).await? {
                if let Some(status) = pod.status {
                    let ready = status
                        .conditions
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .any(|condition| condition.type_ == "Ready" && condition.status == "True");
                    if ready {
                        let image_id = status
                            .container_statuses
                            .as_deref()
                            .unwrap_or_default()
                            .iter()
                            .find(|container| container.name == "workspace")
                            .map(|container| container.image_id.clone())
                            .filter(|value| !value.is_empty())
                            .context("ready workspace container has no resolved imageID")?;
                        return Ok(Provisioned {
                            image_digest: reusable_image_digest(&workspace.image_ref, &image_id)?,
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

async fn request_supervisor_checkpoint(url: &str, token: &str, timeout: Duration) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("build supervisor checkpoint client")?;
    let response = client
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .context("request final checkpoint before sandbox deletion")?;
    if !response.status().is_success() {
        bail!(
            "supervisor final checkpoint failed with HTTP {}",
            response.status()
        );
    }
    Ok(())
}

#[async_trait]
impl Orchestrator for Kubernetes {
    async fn provision(
        &self,
        workspace: &Workspace,
        project: &Project,
        has_profile: bool,
    ) -> Result<Provisioned> {
        self.reconcile_stable(workspace).await?;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let name = pod_name(workspace);
        if pods.get_opt(&name).await?.is_none() {
            let manifest: Pod = serde_json::from_value(pod_manifest(
                &self.config,
                workspace,
                project,
                has_profile,
            )?)?;
            pods.create(&PostParams::default(), &manifest)
                .await
                .context("create sandbox Pod")?;
        }
        self.wait_ready(workspace).await
    }

    async fn suspend(&self, workspace: &Workspace) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        if let Some(pod) = pods.get_opt(&pod_name(workspace)).await? {
            self.checkpoint_before_delete(workspace, &pod).await?;
            let delete = DeleteParams {
                grace_period_seconds: Some(
                    self.config.lifecycle.termination_grace_seconds.try_into()?,
                ),
                ..DeleteParams::default()
            };
            pods.delete(&pod_name(workspace), &delete).await?;
            let deadline = tokio::time::Instant::now()
                + Duration::from_secs(self.config.lifecycle.termination_grace_seconds + 30);
            while let Some(pod) = pods.get_opt(&pod_name(workspace)).await? {
                if let Some(exit_code) = workspace_exit_code(&pod) {
                    if exit_code != 0 {
                        bail!("sandbox supervisor exited with {exit_code}; final checkpoint may have failed");
                    }
                }
                if tokio::time::Instant::now() >= deadline {
                    bail!("sandbox Pod deletion timed out");
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    async fn delete(&self, workspace: &Workspace) -> Result<()> {
        self.suspend(workspace).await?;
        let services: Api<Service> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.config.namespace);
        if services.get_opt(&workspace.service_name).await?.is_some() {
            services
                .delete(&workspace.service_name, &DeleteParams::default())
                .await?;
        }
        let name = runtime_secret_name(workspace);
        if secrets.get_opt(&name).await?.is_some() {
            secrets.delete(&name, &DeleteParams::default()).await?;
        }
        Ok(())
    }

    async fn put_env_profile(&self, owner: &str, project: &str, content: &[u8]) -> Result<()> {
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let name = env_secret_name(owner, project);
        let mut data = BTreeMap::new();
        data.insert("profile.envrc".into(), ByteString(content.to_vec()));
        let secret = Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.clone()),
                namespace: Some(self.config.namespace.clone()),
                labels: Some(BTreeMap::from([(
                    "opencode.dev/managed-by".into(),
                    "gateway".into(),
                )])),
                ..Default::default()
            },
            immutable: Some(false),
            data: Some(data),
            type_: Some("Opaque".into()),
            ..Default::default()
        };
        secrets
            .patch(
                &name,
                &PatchParams::apply(MANAGER).force(),
                &Patch::Apply(secret),
            )
            .await?;
        Ok(())
    }

    async fn delete_env_profile(&self, owner: &str, project: &str) -> Result<()> {
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let name = env_secret_name(owner, project);
        if secrets.get_opt(&name).await?.is_some() {
            secrets.delete(&name, &DeleteParams::default()).await?;
        }
        Ok(())
    }

    async fn ready_pod_ip(&self, preview_key: &str) -> Option<String> {
        self.pod_ips.read().await.get(preview_key).cloned()
    }

    async fn health(&self) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        pods.list(&ListParams::default().limit(1)).await?;
        Ok(())
    }
}

fn workspace_exit_code(pod: &Pod) -> Option<i32> {
    pod.status
        .as_ref()?
        .container_statuses
        .as_ref()?
        .iter()
        .find(|status| status.name == "workspace")?
        .state
        .as_ref()?
        .terminated
        .as_ref()
        .map(|terminated| terminated.exit_code)
}

async fn update_index(index: &RwLock<HashMap<String, String>>, pod: &Pod) {
    let ready = pod
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .into_iter()
        .flatten()
        .any(|condition| condition.type_ == "Ready" && condition.status == "True");
    if !ready {
        remove_index(index, pod).await;
        return;
    }
    if let (Some(key), Some(ip)) = (
        pod.labels().get("opencode.dev/preview-key"),
        pod.status
            .as_ref()
            .and_then(|status| status.pod_ip.as_ref()),
    ) {
        index.write().await.insert(key.clone(), ip.clone());
    }
}

async fn remove_index(index: &RwLock<HashMap<String, String>>, pod: &Pod) {
    if let Some(key) = pod.labels().get("opencode.dev/preview-key") {
        index.write().await.remove(key);
    }
}

pub fn resource_key(workspace_id: &str) -> String {
    let slug: String = workspace_id
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let hash = format!("{:x}", Sha256::digest(workspace_id.as_bytes()));
    let prefix: String = slug.chars().take(40).collect();
    format!(
        "{}-{}",
        if prefix.is_empty() {
            "workspace"
        } else {
            &prefix
        },
        &hash[..12]
    )
}

fn reusable_image_digest(image_ref: &str, image_id: &str) -> Result<String> {
    if let Some(value) = image_id.strip_prefix("docker-pullable://") {
        if value.contains("@sha256:") {
            return Ok(value.to_owned());
        }
    }
    let digest = image_id
        .find("sha256:")
        .map(|offset| &image_id[offset..])
        .context("container imageID does not contain a SHA-256 digest")?;
    let without_digest = image_ref.split('@').next().unwrap_or(image_ref);
    let slash = without_digest.rfind('/');
    let colon = without_digest.rfind(':');
    let repository = if colon.is_some_and(|colon| slash.is_none_or(|slash| colon > slash)) {
        &without_digest[..colon.unwrap()]
    } else {
        without_digest
    };
    Ok(format!("{repository}@{digest}"))
}

pub fn pod_name(workspace: &Workspace) -> String {
    format!("sandbox-{}", workspace.preview_key)
}
fn runtime_secret_name(workspace: &Workspace) -> String {
    format!("runtime-{}", workspace.preview_key)
}
pub fn env_secret_name(owner: &str, project: &str) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(format!("{owner}\0{project}").as_bytes())
    );
    format!("env-{project}-{}", &hash[..12])
}

fn labels(workspace: &Workspace) -> serde_json::Value {
    json!({ "opencode.dev/managed-by": "gateway", "opencode.dev/preview-key": workspace.preview_key })
}

fn service_manifest(namespace: &str, workspace: &Workspace) -> serde_json::Value {
    json!({ "apiVersion":"v1", "kind":"Service", "metadata": { "name":workspace.service_name, "namespace":namespace, "labels":labels(workspace) }, "spec": { "type":"ClusterIP", "selector": { "opencode.dev/preview-key":workspace.preview_key }, "ports":[{"name":"opencode","port":OPENCODE_PORT,"targetPort":OPENCODE_PORT}] } })
}

fn runtime_secret_manifest(namespace: &str, workspace: &Workspace) -> Result<serde_json::Value> {
    let environment: BTreeMap<String, Option<String>> =
        serde_json::from_str(&workspace.upstream_environment)
            .context("parse upstream environment")?;
    let auth_content = environment
        .get("OPENCODE_AUTH_CONTENT")
        .and_then(Option::as_deref)
        .filter(|value| !value.is_empty())
        .context("upstream environment omitted OPENCODE_AUTH_CONTENT")?;
    Ok(
        json!({ "apiVersion":"v1", "kind":"Secret", "metadata": { "name":runtime_secret_name(workspace), "namespace":namespace, "labels":labels(workspace) }, "type":"Opaque", "stringData": { "password":workspace.password, "runtime-token":workspace.runtime_token, "opencode-auth-content":auth_content } }),
    )
}

pub fn pod_manifest(
    config: &Config,
    workspace: &Workspace,
    project: &Project,
    has_profile: bool,
) -> Result<serde_json::Value> {
    let image = workspace
        .image_digest
        .as_deref()
        .map(|value| value.strip_prefix("docker-pullable://").unwrap_or(value))
        .unwrap_or(&workspace.image_ref);
    let mut volumes = vec![
        json!({"name":"workspace","emptyDir":{}}),
        json!({"name":"runtime","emptyDir":{}}),
        json!({"name":"runtime-state","emptyDir":{}}),
        json!({"name":"runtime-auth","secret":{"secretName":runtime_secret_name(workspace),"defaultMode":288}}),
    ];
    let mut workspace_mounts = vec![
        json!({"name":"workspace","mountPath":"/workspace"}),
        json!({"name":"runtime","mountPath":"/opt/opencode"}),
        json!({"name":"runtime-state","mountPath":"/run/opencode"}),
        json!({"name":"runtime-auth","mountPath":"/run/opencode-auth","readOnly":true}),
    ];
    let mut checkout_mounts = vec![
        json!({"name":"workspace","mountPath":"/workspace"}),
        json!({"name":"runtime-auth","mountPath":"/run/opencode-auth","readOnly":true}),
    ];
    if has_profile {
        volumes.push(json!({"name":"env-profile","secret":{"secretName":env_secret_name(&workspace.owner, &workspace.project_key),"defaultMode":288}}));
        workspace_mounts
            .push(json!({"name":"env-profile","mountPath":"/run/opencode-env","readOnly":true}));
        checkout_mounts
            .push(json!({"name":"env-profile","mountPath":"/run/opencode-env","readOnly":true}));
    }
    let security = json!({"allowPrivilegeEscalation":false,"capabilities":{"drop":["ALL"]}});
    checkout_mounts.push(json!({"name":"runtime","mountPath":"/opt/opencode"}));
    checkout_mounts.push(json!({"name":"runtime-state","mountPath":"/run/opencode"}));
    let profile_script = if has_profile {
        "if git -C /workspace ls-files --error-unmatch .envrc >/dev/null 2>&1; then echo 'tracked .envrc conflicts with private environment profile' >&2; exit 1; fi\nln -s /run/opencode-env/profile.envrc /workspace/.envrc"
    } else {
        ":"
    };
    let checkout_script = format!(
        r#"set -eu
umask 0002
git clone --no-checkout "$REPOSITORY" /workspace
git -C /workspace fetch --depth=1 origin "$GIT_REF"
git -C /workspace checkout --detach FETCH_HEAD
mkdir -p /run/opencode/restore
METADATA=/run/opencode/restore/metadata.json
BUNDLE=/run/opencode/restore/checkpoint.bundle
AUTHORIZATION="Authorization: Bearer $(cat /run/opencode-auth/runtime-token)"
STATUS=$(curl --silent --show-error --output "$METADATA" --write-out '%{{http_code}}' --header "$AUTHORIZATION" "$GATEWAY_URL/v1/workspaces/$OPENCODE_WORKSPACE_ID/checkpoints/latest")
case "$STATUS" in
  200)
    curl --silent --show-error --fail --output "$BUNDLE" --header "$AUTHORIZATION" "$GATEWAY_URL/v1/workspaces/$OPENCODE_WORKSPACE_ID/checkpoints/latest/blob"
    /opt/opencode/bin/supervisor restore --repo /workspace --metadata "$METADATA" --bundle "$BUNDLE"
    ;;
  404) rm -f "$METADATA" ;;
  *) echo "checkpoint metadata request failed with HTTP $STATUS" >&2; exit 1 ;;
esac
{profile_script}
chmod -R a+rwX /workspace /run/opencode"#
    );
    let nix_flake = match &project.environment {
        ProjectEnvironment::Nix { flake } => Some(flake.as_str()),
        _ => None,
    };
    let checkpoint_sidecar = json!({"name":"checkpoint","image":config.runtime.image,"restartPolicy":"Always","command":["/opt/opencode/bin/supervisor","sidecar"],"env":[{"name":"WORKSPACE_PATH","value":"/workspace"},{"name":"OPENCODE_WORKSPACE_ID","value":workspace.id},{"name":"CHECKPOINT_OUTPUT_DIR","value":"/run/opencode/checkpoints"},{"name":"CHECKPOINT_LISTEN","value":format!("127.0.0.1:{CHECKPOINT_PORT}")},{"name":"CHECKPOINT_CONTROL_TOKEN_FILE","value":"/run/opencode-auth/runtime-token"},{"name":"GATEWAY_URL","value":config.runtime.gateway_url},{"name":"WORKSPACE_RUNTIME_TOKEN_FILE","value":"/run/opencode-auth/runtime-token"}],"ports":[{"name":"checkpoint","containerPort":CHECKPOINT_PORT}],"volumeMounts":[{"name":"workspace","mountPath":"/workspace"},{"name":"runtime","mountPath":"/opt/opencode"},{"name":"runtime-state","mountPath":"/run/opencode"},{"name":"runtime-auth","mountPath":"/run/opencode-auth","readOnly":true}],"resources":{"requests":{"cpu":"10m","memory":"32Mi"},"limits":{"cpu":"500m","memory":"256Mi"}},"securityContext":security});
    Ok(json!({
      "apiVersion":"v1", "kind":"Pod", "metadata":{"name":pod_name(workspace),"namespace":config.namespace,"labels":labels(workspace)},
      "spec":{
        "automountServiceAccountToken":false,"enableServiceLinks":false,"restartPolicy":"Never","terminationGracePeriodSeconds":config.lifecycle.termination_grace_seconds,"securityContext":{"seccompProfile":{"type":"RuntimeDefault"},"fsGroup":2000,"fsGroupChangePolicy":"OnRootMismatch"},
        "volumes":volumes,
        "initContainers":[
          {"name":"runtime-inject","image":config.runtime.image,"command":["/bin/sh","-ec","umask 0002; cp -a /opt/opencode/. /runtime/; chmod -R a+rX,g+w /runtime"],"volumeMounts":[{"name":"runtime","mountPath":"/runtime"}],"securityContext":security},
          {"name":"checkout","image":config.runtime.image,"command":["/bin/sh","-ec",checkout_script],"env":[{"name":"REPOSITORY","value":project.repository},{"name":"GIT_REF","value":workspace.git_ref},{"name":"GATEWAY_URL","value":config.runtime.gateway_url},{"name":"OPENCODE_WORKSPACE_ID","value":workspace.id}],"volumeMounts":checkout_mounts,"securityContext":security},
          checkpoint_sidecar
        ],
        "containers":[
          {"name":"workspace","image":image,"imagePullPolicy":"IfNotPresent","command":["/opt/opencode/bin/supervisor"],"workingDir":"/workspace","ports":[{"name":"opencode","containerPort":OPENCODE_PORT}],"env":[
            {"name":"OPENCODE_WORKSPACE_ID","value":workspace.id},{"name":"OPENCODE_EXPERIMENTAL_WORKSPACES","value":"true"},{"name":"OPENCODE_CONFIG_DIR","value":"/opt/opencode/config"},{"name":"OPENCODE_SERVER_USERNAME","value":"opencode"},{"name":"OPENCODE_SERVER_PASSWORD","valueFrom":{"secretKeyRef":{"name":runtime_secret_name(workspace),"key":"password"}}},{"name":"OPENCODE_AUTH_CONTENT","valueFrom":{"secretKeyRef":{"name":runtime_secret_name(workspace),"key":"opencode-auth-content"}}},{"name":"OPENCODE_EXPECTED_VERSION","value":config.opencode.version},{"name":"OPENCODE_GATEWAY_URL","value":config.runtime.gateway_url},{"name":"OPENCODE_GATEWAY_TOKEN","valueFrom":{"secretKeyRef":{"name":runtime_secret_name(workspace),"key":"runtime-token"}}},{"name":"OPENCODE_BASE_DOMAIN","value":config.base_domain},{"name":"OPENCODE_SUPERVISOR_ENDPOINT","value":format!("http://127.0.0.1:{SUPERVISOR_PORT}")},{"name":"OPENCODE_CHECKPOINT_ENDPOINT","value":format!("http://127.0.0.1:{CHECKPOINT_PORT}")},{"name":"OPENCODE_DIRENV_PATH","value":"/opt/opencode/bin/direnv"},{"name":"SUPERVISOR_LISTEN","value":format!("127.0.0.1:{SUPERVISOR_PORT}")},{"name":"SUPERVISOR_CONTROL_TOKEN_FILE","value":"/run/opencode-auth/runtime-token"},{"name":"CHECKPOINT_SIDECAR_URL","value":format!("http://127.0.0.1:{CHECKPOINT_PORT}/checkpoint")},{"name":"CHECKPOINT_CONTROL_TOKEN_FILE","value":"/run/opencode-auth/runtime-token"},{"name":"OPENCODE_NIX_FLAKE","value":nix_flake}
          ],"volumeMounts":workspace_mounts,"resources":{"requests":{"cpu":project.resources.requests.cpu,"memory":project.resources.requests.memory},"limits":{"cpu":project.resources.limits.cpu,"memory":project.resources.limits.memory}},"readinessProbe":{"tcpSocket":{"port":OPENCODE_PORT},"periodSeconds":2,"failureThreshold":150},"securityContext":security}
        ]
      }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{
            AuthConfig, CheckpointConfig, LifecycleConfig, OpenCodeConfig, ResourcePair, Resources,
            RuntimeConfig,
        },
        state::WorkspaceState,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn pod_is_ephemeral_and_hardened() {
        let config = Config {
            namespace: "sandboxes".into(),
            base_domain: "test.invalid".into(),
            listen: "x".into(),
            preview_listen: "y".into(),
            state_path: "z".into(),
            opencode: OpenCodeConfig {
                version: "1.18.3".into(),
                central_url: "x".into(),
            },
            runtime: RuntimeConfig {
                image: "runtime:v1".into(),
                generic_nix_image: "nix:v1".into(),
                gateway_url: "http://gateway:8080".into(),
            },
            checkpoint: CheckpointConfig {
                path: "x".into(),
                periodic_seconds: 120,
            },
            lifecycle: LifecycleConfig {
                suspend_after_idle_seconds: 3600,
                ready_timeout_seconds: 30,
                poll_seconds: 30,
                termination_grace_seconds: 180,
            },
            auth: AuthConfig::Development {
                user: "dev".into(),
                internal_token_file: None,
            },
            projects_file: "x".into(),
        };
        let workspace = Workspace {
            id: "wrk_1".into(),
            project_key: "demo".into(),
            git_ref: "main".into(),
            owner: "dev".into(),
            state: WorkspaceState::Provisioning,
            service_name: "workspace-a".into(),
            preview_key: "a".into(),
            password: "secret".into(),
            runtime_token: "token".into(),
            upstream_environment: r#"{"OPENCODE_AUTH_CONTENT":"auth-json"}"#.into(),
            image_ref: "demo:dev".into(),
            image_digest: None,
            last_activity: String::new(),
            error: None,
        };
        let project = Project {
            key: "demo".into(),
            name: "Demo".into(),
            repository: "https://git/demo".into(),
            default_ref: "main".into(),
            environment: ProjectEnvironment::Image {
                image: "demo:dev".into(),
            },
            resources: Resources {
                requests: ResourcePair {
                    cpu: "100m".into(),
                    memory: "128Mi".into(),
                },
                limits: ResourcePair {
                    cpu: "1".into(),
                    memory: "1Gi".into(),
                },
            },
        };
        let pod = pod_manifest(&config, &workspace, &project, true).unwrap();
        assert!(pod["spec"]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|volume| volume.get("emptyDir").is_some() || volume.get("secret").is_some()));
        assert_eq!(pod["spec"]["automountServiceAccountToken"], false);
        assert_eq!(pod["spec"]["securityContext"]["fsGroup"], 2000);
        assert_eq!(
            pod["spec"]["securityContext"]["seccompProfile"]["type"],
            "RuntimeDefault"
        );
        assert!(pod.to_string().contains("allowPrivilegeEscalation"));
        assert!(!pod.to_string().contains("persistentVolumeClaim"));

        let init = pod["spec"]["initContainers"].as_array().unwrap();
        assert_eq!(init[0]["name"], "runtime-inject");
        assert_eq!(init[1]["name"], "checkout");
        let checkout = init[1].to_string();
        assert!(checkout.contains("/opt/opencode/bin/supervisor restore"));
        assert!(checkout.contains("checkpoints/latest/blob"));
        assert!(checkout.contains("Authorization: Bearer"));
        assert!(!checkout.contains("CHECKPOINT_HEAD"));
        assert!(init[1]["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mount| mount["mountPath"] == "/opt/opencode"));

        let containers = pod["spec"]["containers"].as_array().unwrap();
        assert_eq!(
            containers[0]["command"],
            json!(["/opt/opencode/bin/supervisor"])
        );
        let main = containers[0].to_string();
        for required in [
            "OPENCODE_AUTH_CONTENT",
            "OPENCODE_GATEWAY_URL",
            "OPENCODE_GATEWAY_TOKEN",
            "OPENCODE_BASE_DOMAIN",
            "OPENCODE_SUPERVISOR_ENDPOINT",
            "OPENCODE_CHECKPOINT_ENDPOINT",
            "SUPERVISOR_CONTROL_TOKEN_FILE",
            "CHECKPOINT_SIDECAR_URL",
            "CHECKPOINT_CONTROL_TOKEN_FILE",
        ] {
            assert!(main.contains(required), "missing {required}");
        }
        assert!(main.contains("http://127.0.0.1:4097"));
        assert!(main.contains("http://127.0.0.1:4098/checkpoint"));
        assert!(main.contains("opencode-auth-content"));
        assert_eq!(
            containers[0]["readinessProbe"]["tcpSocket"]["port"],
            OPENCODE_PORT
        );
        assert!(containers[0]["readinessProbe"].get("httpGet").is_none());
        assert!(!main.contains("auth-json"));

        assert_eq!(containers.len(), 1);
        assert_eq!(
            init[2]["command"],
            json!(["/opt/opencode/bin/supervisor", "sidecar"])
        );
        assert_eq!(init[2]["restartPolicy"], "Always");
        let sidecar = init[2].to_string();
        for required in [
            "CHECKPOINT_LISTEN",
            "127.0.0.1:4098",
            "CHECKPOINT_CONTROL_TOKEN_FILE",
            "WORKSPACE_RUNTIME_TOKEN_FILE",
            "GATEWAY_URL",
            "/run/opencode/checkpoints",
        ] {
            assert!(sidecar.contains(required), "missing {required}");
        }
        let secret = runtime_secret_manifest("sandboxes", &workspace).unwrap();
        assert_eq!(secret["stringData"]["opencode-auth-content"], "auth-json");
        assert!(secret.to_string().contains("runtime-token"));
    }

    #[test]
    fn resource_mapping_is_stable_and_collision_safe() {
        assert_eq!(resource_key("wrk_1"), resource_key("wrk_1"));
        assert_ne!(resource_key("wrk_1"), resource_key("wrk-1"));
    }

    #[test]
    fn records_a_reusable_digest_from_containerd_status() {
        assert_eq!(
            reusable_image_digest("registry.test/team/demo:dev", "containerd://sha256:abc")
                .unwrap(),
            "registry.test/team/demo@sha256:abc"
        );
    }

    #[test]
    fn observes_failed_supervisor_termination() {
        let pod: Pod = serde_json::from_value(json!({
            "metadata": {},
            "status": {"containerStatuses": [{
                "name": "workspace", "image": "demo", "imageID": "demo",
                "ready": false, "restartCount": 0,
                "state": {"terminated": {"exitCode": 1, "reason": "Error", "startedAt": null, "finishedAt": null}}
            }]}
        })).unwrap();
        assert_eq!(workspace_exit_code(&pod), Some(1));
    }

    #[tokio::test]
    async fn supervisor_checkpoint_request_is_authenticated_and_awaited() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let app = axum::Router::new().route(
                "/checkpoint",
                axum::routing::post(move |headers: axum::http::HeaderMap| {
                    let observed = Arc::clone(&observed);
                    async move {
                        if headers.get(axum::http::header::AUTHORIZATION)
                            != Some(&axum::http::HeaderValue::from_static(
                                "Bearer runtime-token",
                            ))
                        {
                            return axum::http::StatusCode::UNAUTHORIZED;
                        }
                        observed.fetch_add(1, Ordering::SeqCst);
                        axum::http::StatusCode::NO_CONTENT
                    }
                }),
            );
            axum::serve(listener, app).await.unwrap();
        });
        request_supervisor_checkpoint(
            &format!("http://{address}/checkpoint"),
            "runtime-token",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let error = request_supervisor_checkpoint(
            &format!("http://{address}/checkpoint"),
            "wrong-token",
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("HTTP 401"));
        server.abort();
    }
}
