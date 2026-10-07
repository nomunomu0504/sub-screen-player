//! Access control.
//!
//! - With a token configured, every request needs `Authorization: Bearer <token>` or
//!   `?token=<token>` (for WebSocket clients in browsers, which cannot set headers).
//! - Without a token the daemon only listens on loopback (enforced by the config). Requests
//!   whose `Host` or `Origin` is not loopback are refused, so web pages cannot reach the API
//!   through the user's browser (CSRF, DNS rebinding).

use std::net::IpAddr;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{ApiError, AppState};

pub async fn guard(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let allowed = match &app.token {
        Some(token) => {
            presented_token(&request).is_some_and(|t| same(t.as_bytes(), token.as_bytes()))
        }
        None => local_browser_context(request.headers()),
    };
    if allowed {
        next.run(request).await
    } else if app.token.is_some() {
        ApiError::new(StatusCode::UNAUTHORIZED, "missing or wrong token").into_response()
    } else {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "requests from web pages are not allowed",
        )
        .into_response()
    }
}

fn presented_token(request: &Request) -> Option<&str> {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    bearer.or_else(|| {
        request
            .uri()
            .query()?
            .split('&')
            .find_map(|pair| pair.strip_prefix("token="))
    })
}

/// Compares without an early exit, so timing does not reveal the token.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn local_browser_context(headers: &HeaderMap) -> bool {
    let value = |name| headers.get(name).map(|v| v.to_str().unwrap_or_default());
    let host_ok = value(header::HOST).is_none_or(is_loopback_host);
    let origin_ok = value(header::ORIGIN).is_none_or(is_loopback_origin);
    host_ok && origin_ok
}

/// `localhost`, `127.0.0.1:7920`, `[::1]:7920`, ...
fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `http://localhost:3000`, ... (`null` and other schemes are refused)
fn is_loopback_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https")
        && is_loopback_host(rest.split('/').next().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_loopback_hosts() {
        for host in [
            "localhost",
            "LOCALHOST:7920",
            "127.0.0.1",
            "127.0.0.1:7920",
            "[::1]:7920",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.com",
            "192.168.1.2:7920",
            "localhost.evil.com",
            "[::2]:1",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn recognizes_loopback_origins() {
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(is_loopback_origin("https://127.0.0.1"));
        assert!(!is_loopback_origin("null"));
        assert!(!is_loopback_origin("https://evil.example"));
        assert!(!is_loopback_origin("file://localhost"));
    }

    #[test]
    fn compares_tokens() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"abcd"));
    }
}
