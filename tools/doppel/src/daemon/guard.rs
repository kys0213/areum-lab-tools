//! Request guard for the loopback TCP listener (docs/doppel-protocol.md §6).
//! A browser visiting another site can still reach 127.0.0.1, so the TCP
//! side only serves requests that are provably from the doppel page:
//!
//! - `Host` must be this listener (`127.0.0.1:<port>` or `localhost:<port>`),
//!   which defeats DNS rebinding;
//! - an `Origin`, when present, must be the same origin, which covers
//!   cross-site WebSocket opens and `fetch` calls;
//! - state-changing requests must carry `X-Doppel-Client`. A cross-origin
//!   page can only send that header after a CORS preflight, and doppel
//!   answers no preflight.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub(super) const CLIENT_HEADER: &str = "x-doppel-client";

pub(super) async fn loopback_only(
    State(port): State<u16>,
    request: Request,
    next: Next,
) -> Response {
    match verdict(request.method(), request.headers(), port) {
        Ok(()) => next.run(request).await,
        Err(reason) => (StatusCode::FORBIDDEN, reason).into_response(),
    }
}

fn verdict(method: &Method, headers: &HeaderMap, port: u16) -> Result<(), &'static str> {
    let allowed_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .ok_or("missing Host")?;
    if !allowed_hosts.iter().any(|h| h == host) {
        return Err("unexpected Host");
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().map_err(|_| "unreadable Origin")?;
        if !allowed_hosts
            .iter()
            .any(|h| origin == format!("http://{h}"))
        {
            return Err("cross-origin request");
        }
    }
    let changes_state = !matches!(*method, Method::GET | Method::HEAD);
    if changes_state && !headers.contains_key(CLIENT_HEADER) {
        return Err("missing X-Doppel-Client");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn same_origin_page_requests_pass() {
        let h = headers(&[
            ("host", "127.0.0.1:7319"),
            ("origin", "http://127.0.0.1:7319"),
            ("x-doppel-client", "ui"),
        ]);
        assert_eq!(verdict(&Method::POST, &h, 7319), Ok(()));
        assert_eq!(
            verdict(&Method::GET, &headers(&[("host", "localhost:7319")]), 7319),
            Ok(())
        );
    }

    #[test]
    fn rebinding_hosts_are_refused() {
        let h = headers(&[("host", "evil.example:7319")]);
        assert_eq!(verdict(&Method::GET, &h, 7319), Err("unexpected Host"));
        assert_eq!(
            verdict(&Method::GET, &HeaderMap::new(), 7319),
            Err("missing Host")
        );
    }

    #[test]
    fn cross_origin_requests_are_refused_even_for_get() {
        let h = headers(&[
            ("host", "127.0.0.1:7319"),
            ("origin", "https://evil.example"),
        ]);
        assert_eq!(verdict(&Method::GET, &h, 7319), Err("cross-origin request"));
    }

    #[test]
    fn writes_without_the_client_header_are_refused() {
        let h = headers(&[("host", "127.0.0.1:7319")]);
        assert_eq!(
            verdict(&Method::POST, &h, 7319),
            Err("missing X-Doppel-Client")
        );
    }
}
