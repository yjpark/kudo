//! External MCP calls park in the context store until one single-use verdict.
//!
//! implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de@fdae3bbd25e0ab56206b07c5d58d6b105f7a32fa28ba95b81e635252c3295ef2

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use kudo_context::{ContextStore, MCP_APPROVAL_TIMEOUT, McpVerdict};

const T0: i64 = 1_792_500_000;

fn store() -> (ContextStore, kudo_context::ContextId) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kudo-context-mcp-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = ContextStore::init(&dir).unwrap();
    let ctx = store.create_context("mcp", "# mcp\n", T0).unwrap();
    (store, ctx)
}

#[test]
fn a_request_waits_with_its_lease_tool_and_arguments_until_allowed() {
    let (store, ctx) = store();
    let request = store
        .request_mcp(
            &ctx,
            "lease-a",
            "mcp__chat__send_message",
            r#"{"text":"hello"}"#,
            T0,
        )
        .unwrap();
    assert_eq!(request.context, ctx);
    assert_eq!(request.lease, "lease-a");
    assert_eq!(request.tool, "mcp__chat__send_message");
    assert_eq!(request.arguments, r#"{"text":"hello"}"#);
    assert_eq!(request.deadline, T0 + MCP_APPROVAL_TIMEOUT);

    let pending = store.pending_mcp(T0 + 1).unwrap();
    assert_eq!(pending, vec![request.clone()]);
    assert_eq!(store.mcp_verdict(&ctx, &request.id, T0 + 1).unwrap(), None);

    store
        .decide_mcp(&ctx, &request.id, McpVerdict::Allow, T0 + 2)
        .unwrap();
    assert_eq!(
        store.mcp_verdict(&ctx, &request.id, T0 + 3).unwrap(),
        Some(McpVerdict::Allow)
    );
    assert!(store.pending_mcp(T0 + 3).unwrap().is_empty());
    assert!(
        store
            .decide_mcp(&ctx, &request.id, McpVerdict::Deny, T0 + 4)
            .is_err(),
        "a request accepts only one verdict"
    );
}

#[test]
fn denial_and_expiry_remove_requests_without_allowing_them() {
    let (store, ctx) = store();
    let denied = store
        .request_mcp(&ctx, "lease-a", "mcp__x__write", "{}", T0)
        .unwrap();
    store
        .decide_mcp(&ctx, &denied.id, McpVerdict::Deny, T0 + 1)
        .unwrap();
    assert_eq!(
        store.mcp_verdict(&ctx, &denied.id, T0 + 2).unwrap(),
        Some(McpVerdict::Deny)
    );

    let expired = store
        .request_mcp(&ctx, "lease-a", "mcp__x__delete", "{}", T0)
        .unwrap();
    assert!(
        store
            .pending_mcp(T0 + MCP_APPROVAL_TIMEOUT)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .mcp_verdict(&ctx, &expired.id, T0 + MCP_APPROVAL_TIMEOUT)
            .unwrap(),
        None
    );
    assert!(
        store
            .decide_mcp(
                &ctx,
                &expired.id,
                McpVerdict::Allow,
                T0 + MCP_APPROVAL_TIMEOUT
            )
            .is_err(),
        "too late is not an approval"
    );
}

#[test]
fn requests_and_verdicts_must_stay_in_their_context() {
    let (store, ctx) = store();
    let other = store.create_context("other", "# other\n", T0).unwrap();
    let request = store
        .request_mcp(&ctx, "lease-a", "mcp__x__write", "{}", T0)
        .unwrap();
    assert!(
        store
            .decide_mcp(&other, &request.id, McpVerdict::Allow, T0 + 1)
            .is_err()
    );
    assert!(store.mcp_verdict(&other, &request.id, T0 + 1).is_err());
}
