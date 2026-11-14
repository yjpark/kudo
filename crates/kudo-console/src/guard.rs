//! What the console server accepts: same-origin JSON only.
//!
//! The console has no login (v1 runs by hand, bound to localhost). What it
//! must refuse is a request another web page could make the person's browser
//! send: a form post or a no-cors fetch can carry only simple content types,
//! and always says where it came from.
//!
//! implements: d7562b1ac2f15d346f0a98561dc3da3351fabcd807f4ce4bbcd614f2f2a73c1f@9ac5369cd207a2da44b4ac1698c0b6725ef7ab4d94798e3411253a0c60d32c29

use dioxus::fullstack::{HeaderMap, Method};

/// The names in a comma-separated variable.
fn listed(var: &str) -> Vec<String> {
    std::env::var(var)
        .map(|list| {
            list.split(',')
                .map(|h| h.trim().to_owned())
                .filter(|h| !h.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `host` (with its port) is one this console answers to: localhost,
/// or one of `hosts`.
fn answers_to(host: &str, hosts: &[String]) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "localhost" | "127.0.0.1" | "[::1]")
        || hosts.iter().any(|h| h == name || h == host)
}

/// Whether a request may reach a server function, with the hosts and
/// origins named in `KUDO_CONSOLE_HOSTS` and `KUDO_CONSOLE_ORIGINS`
/// (comma-separated) — for a console reached through an ingress, whose Host
/// is the proxy's and whose Origin is the page's.
pub fn allowed(method: &Method, headers: &HeaderMap) -> Result<(), String> {
    allowed_with(
        method,
        headers,
        &listed("KUDO_CONSOLE_HOSTS"),
        &listed("KUDO_CONSOLE_ORIGINS"),
    )
}

/// [`allowed`], with the named hosts and origins given. A page load (GET or
/// HEAD, in which the page's own render runs the server functions) writes
/// nothing another site could aim; anything else must name a host this
/// console answers to (which defeats DNS rebinding), be JSON, and come from
/// the console's own origin or one named.
pub fn allowed_with(
    method: &Method,
    headers: &HeaderMap,
    hosts: &[String],
    origins: &[String],
) -> Result<(), String> {
    if method == Method::GET || method == Method::HEAD {
        return Ok(());
    }
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let host = get("host").unwrap_or_default();
    if !answers_to(host, hosts) {
        return Err(format!(
            "the console does not answer to {host:?}; add it to KUDO_CONSOLE_HOSTS"
        ));
    }
    if !get("content-type").is_some_and(|ct| ct.starts_with("application/json")) {
        return Err("the console takes JSON only".to_owned());
    }
    if get("sec-fetch-site").is_some_and(|site| site != "same-origin" && site != "none") {
        return Err("the console refuses requests from another site".to_owned());
    }
    if let Some(origin) = get("origin") {
        let own = origin == format!("http://{host}") || origin == format!("https://{host}");
        if !own && !origins.iter().any(|o| o == origin) {
            return Err(format!("the console refuses requests from {origin}"));
        }
    }
    Ok(())
}
