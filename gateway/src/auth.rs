use std::{fs, sync::Arc};

use anyhow::{bail, Context, Result};
use axum::http::{header, HeaderMap};
use subtle::ConstantTimeEq;

use crate::config::AuthConfig;

#[derive(Clone)]
pub struct Authenticator {
    config: AuthConfig,
    internal_token: Option<Arc<str>>,
}

impl Authenticator {
    pub fn new(config: &AuthConfig) -> Result<Self> {
        let path = match config {
            AuthConfig::TrustedHeader {
                internal_token_file,
                ..
            }
            | AuthConfig::Development {
                internal_token_file,
                ..
            } => internal_token_file,
        };
        let internal_token = path
            .as_deref()
            .map(|path| {
                let value = fs::read_to_string(path)
                    .with_context(|| format!("read auth.internalTokenFile {path}"))?;
                let value = value.trim();
                if value.is_empty() {
                    bail!("auth.internalTokenFile must not be empty");
                }
                Ok::<Arc<str>, anyhow::Error>(Arc::from(value))
            })
            .transpose()?;
        Ok(Self {
            config: config.clone(),
            internal_token,
        })
    }

    pub fn identity(&self, headers: &HeaderMap) -> Result<String, &'static str> {
        let value = match &self.config {
            AuthConfig::Development { user, .. } => user.clone(),
            AuthConfig::TrustedHeader {
                identity_header, ..
            } => headers
                .get(identity_header)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
                .ok_or("missing authenticated identity")?,
        };
        validate_identity(&value)
    }

    pub fn is_internal(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.internal_token else {
            return false;
        };
        let Some(candidate) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        candidate.as_bytes().ct_eq(expected.as_bytes()).into()
    }
}

pub fn validate_identity(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 254 || value.chars().any(char::is_control) {
        return Err("invalid authenticated identity");
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn maps_trusted_header_and_internal_token() {
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        fs::write(&token, " service-secret\n").unwrap();
        let auth = Authenticator::new(&AuthConfig::TrustedHeader {
            identity_header: "x-user".into(),
            internal_token_file: Some(token.to_string_lossy().into()),
        })
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-user", HeaderValue::from_static("dev@example.test"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer service-secret"),
        );
        assert_eq!(auth.identity(&headers).unwrap(), "dev@example.test");
        assert!(auth.is_internal(&headers));
    }

    #[test]
    fn configured_missing_or_empty_internal_token_fails_startup() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(Authenticator::new(&AuthConfig::Development {
            user: "dev".into(),
            internal_token_file: Some(missing.to_string_lossy().into()),
        })
        .is_err());
        let empty = dir.path().join("empty");
        fs::write(&empty, " \n").unwrap();
        assert!(Authenticator::new(&AuthConfig::Development {
            user: "dev".into(),
            internal_token_file: Some(empty.to_string_lossy().into()),
        })
        .is_err());
    }
}
