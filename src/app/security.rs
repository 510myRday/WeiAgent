use std::{collections::HashSet, net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiSecurity {
    token: String,
    allowed_hosts: HashSet<String>,
    allowed_origins: HashSet<String>,
    enabled: bool,
}

impl ApiSecurity {
    pub fn new(address: SocketAddr, development: bool) -> Self {
        let port = address.port();
        let allowed_hosts = [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
            address.to_string(),
        ]
        .into_iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>();
        let mut allowed_origins = allowed_hosts
            .iter()
            .map(|host| format!("http://{host}"))
            .collect::<HashSet<_>>();
        if development {
            allowed_origins.insert("http://127.0.0.1:5173".to_string());
            allowed_origins.insert("http://localhost:5173".to_string());
        }
        Self {
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            allowed_hosts,
            allowed_origins,
            enabled: true,
        }
    }

    #[cfg(test)]
    pub fn disabled_for_tests() -> Arc<Self> {
        Arc::new(Self {
            token: String::new(),
            allowed_hosts: HashSet::new(),
            allowed_origins: HashSet::new(),
            enabled: false,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    fn validate(&self, request: &Request<Body>) -> Result<(), StatusCode> {
        if !self.enabled {
            return Ok(());
        }
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(str::to_ascii_lowercase)
            .ok_or(StatusCode::BAD_REQUEST)?;
        if !self.allowed_hosts.contains(&host) {
            return Err(StatusCode::FORBIDDEN);
        }
        if let Some(origin) = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            && !self.allowed_origins.contains(&origin.to_ascii_lowercase())
        {
            return Err(StatusCode::FORBIDDEN);
        }
        let bearer = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .or_else(|| {
                matches!(
                    request.uri().path(),
                    "/api/assets/local" | "/api/assets/image" | "/api/assets/remote"
                )
                .then(|| query_token(request.uri().query().unwrap_or_default()))
                .flatten()
            })
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if !constant_time_eq(bearer.as_bytes(), self.token.as_bytes()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }
}

fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == "token" && !value.is_empty()).then_some(value)
    })
}

pub async fn enforce_api_security(
    State(security): State<Arc<ApiSecurity>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match security.validate(&request) {
        Ok(()) => next.run(request).await,
        Err(status) => (
            status,
            status.canonical_reason().unwrap_or("request rejected"),
        )
            .into_response(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(host: &str, origin: Option<&str>, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .uri("/api/settings")
            .header(header::HOST, host);
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn constant_time_comparison_matches_equal_tokens() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"other"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn generated_tokens_are_random_and_fixed_length() {
        let address = "127.0.0.1:9527".parse().unwrap();
        let first = ApiSecurity::new(address, false);
        let second = ApiSecurity::new(address, false);

        assert_eq!(first.token().len(), 64);
        assert_ne!(first.token(), second.token());
    }

    #[test]
    fn validates_bearer_host_and_origin_together() {
        let security = ApiSecurity::new("127.0.0.1:9527".parse().unwrap(), false);
        assert_eq!(
            security.validate(&request(
                "127.0.0.1:9527",
                Some("http://127.0.0.1:9527"),
                Some(security.token()),
            )),
            Ok(())
        );
        assert_eq!(
            security.validate(&request(
                "attacker.invalid",
                Some("http://127.0.0.1:9527"),
                Some(security.token()),
            )),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            security.validate(&request(
                "127.0.0.1:9527",
                Some("https://attacker.invalid"),
                Some(security.token()),
            )),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            security.validate(&request(
                "127.0.0.1:9527",
                Some("http://127.0.0.1:9527"),
                Some("wrong-token"),
            )),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            security.validate(&request(
                "127.0.0.1:9527",
                Some("http://127.0.0.1:9527"),
                None,
            )),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn accepts_query_token_only_for_local_assets() {
        let security = ApiSecurity::new("127.0.0.1:9527".parse().unwrap(), false);
        let asset_request = Request::builder()
            .uri("/api/assets/local?path=C%3A%5Cimage.png&token=test-token")
            .header(header::HOST, "127.0.0.1:9527")
            .body(Body::empty())
            .unwrap();
        let api_request = Request::builder()
            .uri("/api/settings?token=test-token")
            .header(header::HOST, "127.0.0.1:9527")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            security.validate(&asset_request),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            security.validate(&api_request),
            Err(StatusCode::UNAUTHORIZED)
        );

        let asset_request = Request::builder()
            .uri(format!(
                "/api/assets/local?path=C%3A%5Cimage.png&token={}",
                security.token()
            ))
            .header(header::HOST, "127.0.0.1:9527")
            .body(Body::empty())
            .unwrap();
        assert_eq!(security.validate(&asset_request), Ok(()));
    }
}
