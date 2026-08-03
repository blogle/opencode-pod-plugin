use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub namespace: String,
    pub base_domain: String,
    #[serde(default = "default_api_listen")]
    pub listen: String,
    #[serde(default = "default_preview_listen")]
    pub preview_listen: String,
    #[serde(default = "default_state_path")]
    pub state_path: String,
    pub opencode: OpenCodeConfig,
    pub runtime: RuntimeConfig,
    pub checkpoint: CheckpointConfig,
    pub lifecycle: LifecycleConfig,
    pub auth: AuthConfig,
    pub projects_file: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenCodeConfig {
    pub version: String,
    pub central_url: String,
    pub public_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConfig {
    pub image: String,
    pub generic_nix_image: String,
    pub gateway_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointConfig {
    pub path: String,
    #[serde(default = "default_checkpoint_seconds")]
    pub periodic_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleConfig {
    #[serde(default = "default_suspend_seconds")]
    pub suspend_after_idle_seconds: u64,
    #[serde(default = "default_ready_seconds")]
    pub ready_timeout_seconds: u64,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    #[serde(default = "default_termination_grace_seconds")]
    pub termination_grace_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AuthConfig {
    TrustedHeader {
        #[serde(rename = "identityHeader")]
        identity_header: String,
        #[serde(default, rename = "internalTokenFile")]
        internal_token_file: Option<String>,
    },
    Development {
        user: String,
        #[serde(default, rename = "internalTokenFile")]
        internal_token_file: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectsFile {
    projects: BTreeMap<String, Project>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    #[serde(skip_deserializing)]
    pub key: String,
    pub name: String,
    pub repository: String,
    pub default_ref: String,
    #[serde(default = "default_profile_target")]
    pub profile_target: String,
    #[serde(default)]
    pub trust_tracked_envrc: bool,
    pub environment: ProjectEnvironment,
    pub resources: Resources,
}

fn default_profile_target() -> String {
    ".envrc".into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProjectEnvironment {
    Image { image: String },
    Nix { flake: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub requests: ResourcePair,
    pub limits: ResourcePair,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcePair {
    pub cpu: String,
    pub memory: String,
}

fn default_api_listen() -> String {
    "0.0.0.0:8080".into()
}
fn default_preview_listen() -> String {
    "0.0.0.0:8081".into()
}
fn default_state_path() -> String {
    "/var/lib/opencode-sandbox/gateway.sqlite3".into()
}
fn default_checkpoint_seconds() -> u64 {
    120
}
fn default_suspend_seconds() -> u64 {
    3600
}
fn default_ready_seconds() -> u64 {
    300
}
fn default_poll_seconds() -> u64 {
    30
}
fn default_termination_grace_seconds() -> u64 {
    180
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read platform config {}", path.as_ref().display()))?;
        let config: Self = serde_yaml::from_str(&text).context("parse platform YAML")?;
        config.validate()?;
        Ok(config)
    }

    pub fn projects(&self) -> Result<BTreeMap<String, Project>> {
        let text = fs::read_to_string(&self.projects_file)
            .with_context(|| format!("read projects config {}", self.projects_file))?;
        let mut file: ProjectsFile = serde_yaml::from_str(&text).context("parse projects YAML")?;
        if file.projects.is_empty() {
            bail!("projects must not be empty");
        }
        for (key, project) in &mut file.projects {
            validate_key(key, "project key")?;
            project.key.clone_from(key);
            if project.repository.is_empty() || project.default_ref.is_empty() {
                bail!("project {key} repository and defaultRef are required");
            }
            if !matches!(project.profile_target.as_str(), ".envrc" | ".env") {
                bail!("project {key} profileTarget must be .envrc or .env");
            }
        }
        Ok(file.projects)
    }

    fn validate(&self) -> Result<()> {
        validate_key(&self.namespace, "namespace")?;
        if self.base_domain.is_empty() || self.base_domain.starts_with('.') {
            bail!("baseDomain is invalid");
        }
        if self.opencode.version.is_empty() || self.opencode.version == "latest" {
            bail!("opencode.version must be exact");
        }
        for image in [&self.runtime.image, &self.runtime.generic_nix_image] {
            if image.is_empty() || image.ends_with(":latest") {
                bail!("runtime images must be pinned");
            }
        }
        let gateway_uri = self
            .runtime
            .gateway_url
            .parse::<axum::http::Uri>()
            .context("runtime.gatewayUrl is invalid")?;
        if !matches!(gateway_uri.scheme_str(), Some("http" | "https"))
            || gateway_uri.authority().is_none()
            || gateway_uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || gateway_uri.query().is_some()
        {
            bail!("runtime.gatewayUrl must be an HTTP URL");
        }
        validate_http_url(&self.opencode.central_url, "opencode.centralUrl")?;
        validate_http_url(&self.opencode.public_url, "opencode.publicUrl")?;
        if self.checkpoint.periodic_seconds == 0
            || self.lifecycle.suspend_after_idle_seconds == 0
            || self.lifecycle.ready_timeout_seconds == 0
            || self.lifecycle.poll_seconds == 0
            || self.lifecycle.termination_grace_seconds < 30
        {
            bail!("lifecycle durations must be positive");
        }
        if let AuthConfig::TrustedHeader {
            identity_header, ..
        } = &self.auth
        {
            if identity_header.is_empty() {
                bail!("auth.identityHeader is required");
            }
        }
        Ok(())
    }
}

fn validate_http_url(value: &str, name: &str) -> Result<()> {
    let uri = value
        .parse::<axum::http::Uri>()
        .with_context(|| format!("{name} is invalid"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
        || uri
            .path_and_query()
            .is_some_and(|value| value.path() != "/")
        || uri.query().is_some()
    {
        bail!("{name} must be an HTTP URL");
    }
    Ok(())
}

fn validate_key(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("{name} contains unsupported characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_typed_platform_and_projects_yaml() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("projects.yaml");
        fs::write(&projects, "projects:\n  demo:\n    name: Demo\n    repository: https://git.test/demo.git\n    defaultRef: main\n    environment: { mode: image, image: registry.test/demo:dev }\n    resources:\n      requests: { cpu: 100m, memory: 128Mi }\n      limits: { cpu: '1', memory: 1Gi }\n").unwrap();
        let token = dir.path().join("internal-token");
        let yaml = format!("namespace: sandboxes\nbaseDomain: test.invalid\nopencode: {{ version: 1.18.3, centralUrl: 'http://central:4096', publicUrl: 'https://opencode.test' }}\nruntime: {{ image: 'runtime:v1', genericNixImage: 'nix:v1', gatewayUrl: 'http://gateway:8080' }}\ncheckpoint: {{ path: '{}' }}\nlifecycle: {{ suspendAfterIdleSeconds: 60 }}\nauth: {{ mode: development, user: dev@test, internalTokenFile: '{}' }}\nprojectsFile: '{}'\n", token.display(), dir.path().display(), projects.display());
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        let projects = config.projects().unwrap();
        assert_eq!(projects["demo"].default_ref, "main");
        assert_eq!(projects["demo"].profile_target, ".envrc");
        assert!(!projects["demo"].trust_tracked_envrc);
        assert!(matches!(
            config.auth,
            AuthConfig::Development {
                internal_token_file: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn rejects_latest_runtime_image() {
        let yaml = "namespace: sandboxes\nbaseDomain: test.invalid\nopencode: { version: 1.18.3, centralUrl: 'http://central', publicUrl: 'https://opencode.test' }\nruntime: { image: 'runtime:latest', genericNixImage: 'nix:v1', gatewayUrl: 'http://gateway:8080' }\ncheckpoint: { path: /tmp }\nlifecycle: { suspendAfterIdleSeconds: 60 }\nauth: { mode: development, user: dev }\nprojectsFile: projects.yaml\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unsafe_environment_profile_target() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("projects.yaml");
        fs::write(&projects, "projects:\n  demo:\n    name: Demo\n    repository: https://git.test/demo.git\n    defaultRef: main\n    profileTarget: ../secret\n    environment: { mode: image, image: registry.test/demo:dev }\n    resources:\n      requests: { cpu: 100m, memory: 128Mi }\n      limits: { cpu: '1', memory: 1Gi }\n").unwrap();
        let config: Config = serde_yaml::from_str(&format!("namespace: sandboxes\nbaseDomain: test.invalid\nopencode: {{ version: 1.18.3, centralUrl: 'http://central:4096', publicUrl: 'https://opencode.test' }}\nruntime: {{ image: 'runtime:v1', genericNixImage: 'nix:v1', gatewayUrl: 'http://gateway:8080' }}\ncheckpoint: {{ path: /tmp }}\nlifecycle: {{ suspendAfterIdleSeconds: 60 }}\nauth: {{ mode: development, user: dev@test }}\nprojectsFile: '{}'\n", projects.display())).unwrap();

        assert!(config.projects().is_err());
    }
}
