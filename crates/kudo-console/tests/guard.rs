//! The console server refuses what a browser on another site could send: a
//! request that is not JSON, or that comes from another origin.
//!
//! implements: d7562b1ac2f15d346f0a98561dc3da3351fabcd807f4ce4bbcd614f2f2a73c1f@9ac5369cd207a2da44b4ac1698c0b6725ef7ab4d94798e3411253a0c60d32c29

#![cfg(feature = "server")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use dioxus::fullstack::{HeaderMap, HeaderValue, Method};
use kudo_console::guard::allowed;

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(*k, HeaderValue::from_static(v));
    }
    h
}

#[test]
fn same_origin_json_is_allowed() {
    assert!(
        allowed(
            &Method::POST,
            &headers(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:8099"),
                ("origin", "http://127.0.0.1:8099"),
            ])
        )
        .is_ok()
    );
    // A tool on this machine sends no Origin at all.
    assert!(
        allowed(
            &Method::POST,
            &headers(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:8099")
            ])
        )
        .is_ok()
    );
}

#[test]
fn what_another_site_could_send_is_refused() {
    // A form or no-cors fetch can only send these without a preflight.
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        assert!(
            allowed(
                &Method::POST,
                &headers(&[("content-type", ct), ("host", "127.0.0.1:8099")])
            )
            .is_err(),
            "{ct}"
        );
    }
    assert!(
        allowed(&Method::POST, &headers(&[("host", "127.0.0.1:8099")])).is_err(),
        "no content type"
    );
    assert!(
        allowed(
            &Method::POST,
            &headers(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:8099"),
                ("origin", "https://evil.example"),
            ])
        )
        .is_err()
    );
    assert!(
        allowed(
            &Method::POST,
            &headers(&[
                ("content-type", "application/json"),
                ("host", "127.0.0.1:8099"),
                ("sec-fetch-site", "cross-site"),
            ])
        )
        .is_err()
    );
}

/// Rendering the page runs the server functions inside the page's own GET:
/// that is not something another site can turn into a write.
#[test]
fn the_page_render_itself_is_allowed() {
    assert!(allowed(&Method::GET, &headers(&[("host", "127.0.0.1:8099")])).is_ok());
}

/// DNS rebinding makes a hostile page same-origin with itself: the host it
/// names must be one this console answers to.
#[test]
fn a_host_the_console_does_not_answer_to_is_refused() {
    let error = allowed(
        &Method::POST,
        &headers(&[
            ("content-type", "application/json"),
            ("host", "evil.example:8080"),
            ("origin", "http://evil.example:8080"),
            ("sec-fetch-site", "same-origin"),
        ]),
    )
    .expect_err("an unknown host is refused");
    assert!(error.contains("evil.example:8080"), "{error}");
    assert!(error.contains("KUDO_CONSOLE_HOSTS"), "{error}");
    for host in ["localhost:8080", "127.0.0.1:8080", "[::1]:8080"] {
        assert!(
            allowed(
                &Method::POST,
                &headers(&[("content-type", "application/json"), ("host", host)])
            )
            .is_ok(),
            "{host}"
        );
    }
}

/// Behind an ingress the Host is the proxy's and the Origin the page's: both
/// are accepted only when named.
#[test]
fn a_proxied_console_accepts_the_origin_it_is_told_of() {
    let proxied = headers(&[
        ("content-type", "application/json"),
        ("host", "8080.yolo.incus"),
        ("origin", "https://yolo-8080.pc.yjpark.org"),
    ]);
    let hosts = vec!["8080.yolo.incus".to_owned()];
    let origins = vec!["https://yolo-8080.pc.yjpark.org".to_owned()];
    assert!(kudo_console::guard::allowed_with(&Method::POST, &proxied, &hosts, &origins).is_ok());
    assert!(
        kudo_console::guard::allowed_with(&Method::POST, &proxied, &hosts, &[]).is_err(),
        "an origin not named is refused"
    );
    assert!(
        kudo_console::guard::allowed_with(&Method::POST, &proxied, &[], &origins).is_err(),
        "a host not named is refused"
    );
}
