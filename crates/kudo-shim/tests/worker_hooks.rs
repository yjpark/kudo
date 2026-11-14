//! The worker side of the shim: `kudo-shim hook <event>` and the worker's MCP
//! tools, over a real context store and no hub.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use kudo_context::{ContextStore, Established, Submission};
use kudo_shim::hook::{self, HookReply};
use kudo_shim::worker_tools;
use kudo_worker::{ClaudeCode, KICKOFF, WorkerSetup};

const T0: i64 = 1_792_500_000;

fn setup() -> (WorkerSetup, ContextStore) {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "kudo-shim-worker-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = ContextStore::init(&dir).unwrap();
    let context = store
        .create_context("shim", "# shim\nstate: active\n", T0)
        .unwrap();
    let setup = WorkerSetup {
        store_dir: dir,
        context,
        lease: "lease-1".into(),
        claim_ttl: 600,
        max_turns: 10,
        idle_limit: 0,
    };
    (setup, store)
}

/// A submission with just a head and a log line.
fn plain(head: &str, log: &str, base: Option<Vec<String>>) -> Submission {
    Submission {
        head: head.into(),
        log: log.into(),
        base,
        ..Default::default()
    }
}

/// The JSON the CLI sends a hook: the fields the hooks read, plus noise.
fn event(name: &str, extra: &str) -> String {
    format!(
        r#"{{"session_id":"lease-1","transcript_path":"/tmp/t.jsonl","cwd":"/w","hook_event_name":"{name}"{extra}}}"#
    )
}

fn json(reply: &HookReply) -> serde_json::Value {
    assert_eq!(reply.code, 0, "{reply:?}");
    serde_json::from_str(&reply.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", reply.stdout))
}

#[test]
fn session_start_adds_the_head_as_context() {
    let (s, _store) = setup();
    let out = json(&hook::run(
        "session-start",
        &event("SessionStart", r#","source":"startup""#),
        &s,
        T0,
    ));
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let text = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("state: active"), "{text}");
}

#[test]
fn stop_blocks_with_the_next_item_and_allows_when_the_queue_is_empty() {
    let (s, store) = setup();
    store.enqueue(&s.context, "the item", T0).unwrap();

    let out = json(&hook::run(
        "stop",
        &event("Stop", r#","stop_hook_active":false"#),
        &s,
        T0,
    ));
    assert_eq!(out["decision"], "block");
    assert!(
        out["reason"].as_str().unwrap().contains("the item"),
        "{out}"
    );

    let claim = store.held_by(&s.context, &s.lease, T0).unwrap().unwrap();
    worker_tools::submit(
        &s,
        &claim.id.0,
        &plain("# shim\nstate: done\n", "did the item", None),
        T0 + 1,
    )
    .unwrap();

    let allow = hook::run(
        "stop",
        &event("Stop", r#","stop_hook_active":true"#),
        &s,
        T0 + 2,
    );
    assert_eq!(
        allow,
        HookReply {
            stdout: String::new(),
            code: 0
        }
    );
}

#[test]
fn the_kickoff_prompt_keeps_the_lease_and_a_human_prompt_ends_it() {
    let (s, store) = setup();
    store.enqueue(&s.context, "one", T0).unwrap();
    let kick = format!(r#","prompt":{}"#, serde_json::to_string(KICKOFF).unwrap());
    let _ = hook::run(
        "user-prompt-submit",
        &event("UserPromptSubmit", &kick),
        &s,
        T0,
    );
    assert_eq!(
        json(&hook::run("stop", &event("Stop", ""), &s, T0 + 1))["decision"],
        "block"
    );

    let human = r#","prompt":"wait, let me drive""#;
    let _ = hook::run(
        "user-prompt-submit",
        &event("UserPromptSubmit", human),
        &s,
        T0 + 2,
    );
    let after = hook::run("stop", &event("Stop", ""), &s, T0 + 3);
    assert_eq!(after.stdout, "", "the lease ended; the stop is allowed");
}

#[test]
fn pre_compact_ends_the_lease() {
    let (s, store) = setup();
    store.enqueue(&s.context, "one", T0).unwrap();
    let _ = hook::run(
        "pre-compact",
        &event("PreCompact", r#","trigger":"auto""#),
        &s,
        T0,
    );
    assert_eq!(hook::run("stop", &event("Stop", ""), &s, T0 + 1).stdout, "");
}

#[test]
fn session_end_settles_the_lease_without_handing_off_more_work() {
    let (s, store) = setup();
    store.enqueue(&s.context, "one", T0).unwrap();
    let _ = hook::run("stop", &event("Stop", ""), &s, T0);
    assert!(store.held_by(&s.context, &s.lease, T0).unwrap().is_some());

    let ended = hook::run("session-end", &event("SessionEnd", ""), &s, T0 + 1);
    assert_eq!(ended, HookReply::quiet());
    assert!(
        store
            .held_by(&s.context, &s.lease, T0 + 1)
            .unwrap()
            .is_none()
    );
    assert_eq!(hook::run("stop", &event("Stop", ""), &s, T0 + 2).stdout, "");
}

#[test]
fn an_unknown_event_is_refused_loudly() {
    let (s, _store) = setup();
    let reply = hook::run("frobnicate", "{}", &s, T0);
    assert_ne!(reply.code, 0);
}

fn pre_tool(tool: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": arguments,
    })
}

fn permission(reply: &HookReply) -> String {
    json(reply)["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
        .to_owned()
}

// implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de
#[test]
fn kudo_non_mcp_and_read_only_mcp_calls_do_not_wait() {
    let (s, store) = setup();
    for input in [
        pre_tool("mcp__kudo__submit", serde_json::json!({"claim": "c"})),
        pre_tool("Bash", serde_json::json!({"command": "true"})),
        pre_tool("mcp__chat__get_message", serde_json::json!({"id": "m"})),
        pre_tool("mcp__search__query", serde_json::json!({"q": "kudo"})),
    ] {
        let reply = hook::pre_tool_use_with(&input, &s, T0, &ClaudeCode, || T0, || {});
        assert_eq!(permission(&reply), "allow", "{input}");
    }
    assert!(store.pending_mcp(T0).unwrap().is_empty());
}

// implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de
#[test]
fn an_external_mcp_write_waits_for_its_single_use_verdict() {
    let (s, store) = setup();
    let input = pre_tool("mcp__chat__send_message", serde_json::json!({"text": "hello"}));
    let reply = hook::pre_tool_use_with(
        &input,
        &s,
        T0,
        &ClaudeCode,
        || T0 + 1,
        || {
            let pending = store.pending_mcp(T0 + 1).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].tool, "mcp__chat__send_message");
            assert_eq!(pending[0].arguments, r#"{"text":"hello"}"#);
            store
                .decide_mcp(&s.context, &pending[0].id, kudo_context::McpVerdict::Allow, T0 + 1)
                .unwrap();
        },
    );
    assert_eq!(permission(&reply), "allow");
    assert!(store.pending_mcp(T0 + 2).unwrap().is_empty());
}

// implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de
#[test]
fn unknown_mcp_verbs_denial_expiry_and_store_failure_never_allow() {
    let (s, store) = setup();
    let input = pre_tool("mcp__chat__archive", serde_json::json!({"id": "c"}));
    let denied = hook::pre_tool_use_with(&input, &s, T0, &ClaudeCode, || T0 + 1, || {
        let pending = store.pending_mcp(T0 + 1).unwrap();
        store
            .decide_mcp(&s.context, &pending[0].id, kudo_context::McpVerdict::Deny, T0 + 1)
            .unwrap();
    });
    assert_eq!(permission(&denied), "deny");

    let expired = hook::pre_tool_use_with(
        &input,
        &s,
        T0 + 10,
        &ClaudeCode,
        || T0 + 10 + kudo_context::MCP_APPROVAL_TIMEOUT,
        || {},
    );
    assert_eq!(permission(&expired), "deny");

    let mut broken = s.clone();
    broken.store_dir = broken.store_dir.join("missing");
    let failed = hook::run(
        "pre-tool-use",
        &input.to_string(),
        &broken,
        T0,
    );
    assert_eq!(permission(&failed), "deny");
}

#[test]
fn the_tools_pull_submit_and_release() {
    let (s, store) = setup();
    store.enqueue(&s.context, "pulled by tool", T0).unwrap();
    store.enqueue(&s.context, "released by tool", T0).unwrap();

    assert!(
        worker_tools::context_read(&s)
            .unwrap()
            .contains("state: active")
    );

    let pulled = worker_tools::pull(&s, T0).unwrap();
    assert!(pulled.contains("pulled by tool"), "{pulled}");
    let claim = store.held_by(&s.context, &s.lease, T0).unwrap().unwrap();
    assert!(pulled.contains(&claim.id.0), "names the claim: {pulled}");
    worker_tools::submit(&s, &claim.id.0, &plain("new head\n", "log", None), T0 + 1).unwrap();
    assert_eq!(store.head(&s.context).unwrap().body, "new head\n");

    // The next item comes with the Stop handoff, not from a second pull.
    let _ = hook::run("stop", &event("Stop", ""), &s, T0 + 2);
    let second = store
        .held_by(&s.context, &s.lease, T0 + 2)
        .unwrap()
        .unwrap();
    worker_tools::release(&s, &second.id.0, T0 + 3).unwrap();
    assert!(store.has_ready(&s.context, T0 + 4).unwrap());

    assert!(
        worker_tools::pull(&s, T0 + 5)
            .unwrap()
            .contains("released by tool")
    );
}

// implements: 8c1fc5e54c5e48bb4086bd6b27a2d09bf41e100de7c743c6b04fd44b48af3d54
#[test]
fn a_blocked_submit_says_it_waits_without_advancing_the_head() {
    let (s, store) = setup();
    store.enqueue(&s.context, "dependent", T0).unwrap();
    let blocker = store.enqueue(&s.context, "blocker", T0).unwrap();
    let claim = store.pull(&s.context, &s.lease, T0, 600).unwrap().unwrap();
    let mut submission = plain("not the new head\n", "waiting", None);
    submission.blocked_on = vec![blocker];

    let reply = worker_tools::submit(&s, &claim.id.0, &submission, T0 + 1).unwrap();
    assert!(reply.contains("Blocked"), "{reply}");
    assert!(reply.contains("head is unchanged"), "{reply}");
    assert!(reply.contains("End your turn"), "{reply}");
}

#[test]
fn pull_on_an_empty_queue_says_so_and_a_bad_claim_is_an_error() {
    let (s, _store) = setup();
    assert!(
        worker_tools::pull(&s, T0)
            .unwrap()
            .to_lowercase()
            .contains("nothing")
    );
    assert!(worker_tools::submit(&s, "no-such-claim", &plain("h", "l", None), T0).is_err());
}

#[test]
fn after_a_submit_pull_says_to_end_the_turn_and_the_stop_hands_over_the_next() {
    let (s, store) = setup();
    store.enqueue(&s.context, "first", T0).unwrap();
    store.enqueue(&s.context, "second", T0).unwrap();

    let _ = worker_tools::pull(&s, T0).unwrap();
    let claim = store.held_by(&s.context, &s.lease, T0).unwrap().unwrap();
    worker_tools::submit(&s, &claim.id.0, &plain("h1\n", "one", None), T0 + 1).unwrap();

    let again = worker_tools::pull(&s, T0 + 2).unwrap();
    assert!(again.contains("end your turn"), "{again}");
    assert_eq!(
        store.held_by(&s.context, &s.lease, T0 + 2).unwrap(),
        None,
        "nothing claimed"
    );

    let out = json(&hook::run("stop", &event("Stop", ""), &s, T0 + 3));
    assert!(out["reason"].as_str().unwrap().contains("second"), "{out}");
}

#[test]
fn the_inbox_tool_files_a_line_under_the_bound_context() {
    let (s, store) = setup();
    let reply = worker_tools::inbox(&s, "the head template is too long", None, T0).unwrap();
    assert!(reply.contains("filed"), "{reply}");
    let entries = store.inbox(&s.context).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].lease, s.lease);
}

// implements: 3a472b4021f7b7496dd37779f3d0c76cafe1c95e8e80b1b236786a81e4c7d0ab@aaddfc2e7ee067ea5780ce6ee2dfe4587bfb47b0c067b2670a31308c019a7e21
#[test]
fn a_submit_from_a_moved_head_hands_back_the_head_as_it_is_now() {
    let (s, store) = setup();
    store.enqueue(&s.context, "mine", T0).unwrap();
    store.enqueue(&s.context, "theirs", T0).unwrap();
    let _ = worker_tools::pull(&s, T0).unwrap();
    let mine = store.held_by(&s.context, &s.lease, T0).unwrap().unwrap();

    // Another worker takes the second item and submits first.
    let theirs = store.pull(&s.context, "lease-2", T0, 600).unwrap().unwrap();
    store
        .submit(
            &theirs.id,
            &Submission {
                head: "# shim\ntheirs landed\n".into(),
                log: "theirs".into(),
                ..Default::default()
            },
            T0 + 1,
        )
        .unwrap();
    let now = store.head(&s.context).unwrap();

    let refused = worker_tools::submit(
        &s,
        &mine.id.0,
        &plain("# shim\nmine\n", "mine", None),
        T0 + 2,
    )
    .unwrap_err();
    assert!(
        refused.contains("theirs landed"),
        "the head as it is: {refused}"
    );
    assert!(
        refused.contains(&now.version),
        "the version to build on: {refused}"
    );
    assert!(refused.contains("base"), "how to resubmit: {refused}");

    worker_tools::submit(
        &s,
        &mine.id.0,
        &plain(
            "# shim\ntheirs landed\nmine\n",
            "mine",
            Some(vec![now.version.clone()]),
        ),
        T0 + 3,
    )
    .unwrap();
    assert_eq!(
        store.head(&s.context).unwrap().body,
        "# shim\ntheirs landed\nmine\n"
    );
}

// implements: 4e67251bc978f34e7d6ce4278eb1503f9e803e51f0057d4d5954c701830f22b9@1b7ca6a98f08e1312c89a21ba22a046c2f9677965e58dd2fde0cae2756307a74
#[test]
fn the_submit_tool_says_which_lines_landed_as_claimed() {
    let (s, store) = setup();
    store.enqueue(&s.context, "one", T0).unwrap();
    let _ = worker_tools::pull(&s, T0).unwrap();
    let claim = store.held_by(&s.context, &s.lease, T0).unwrap().unwrap();
    let sub = Submission {
        head: "# shim
checked
"
        .into(),
        log: "checked it".into(),
        established: vec![
            Established {
                claim: "backed by a page".into(),
                evidence: vec!["https://example.org/page".into()],
            },
            Established {
                claim: "backed by nothing".into(),
                evidence: vec![],
            },
        ],
        ..Default::default()
    };
    let reply = worker_tools::submit(&s, &claim.id.0, &sub, T0 + 1).unwrap();
    assert!(reply.contains("backed by nothing"), "{reply}");
    assert!(reply.contains("claimed"), "{reply}");
    assert!(!reply.contains("backed by a page"), "{reply}");
}

/// A check's verdict as the `submit` tool takes it: pass / fail / snooze
/// (with when to look again) and a note; anything else is refused.
// implements: c11043084f97c61ff3f41e4d07fb2361aa871213021e9c15d0e4cee37c99bde1
#[test]
fn a_verdict_is_read_from_the_submit_arguments() {
    use kudo_context::{CheckClose, CheckVerdict};
    let close = |v: Option<&str>, until: Option<&str>, note: Option<&str>| {
        worker_tools::check_close(v, until, note, T0)
    };
    assert_eq!(close(None, None, None), Ok(None));
    assert_eq!(
        close(Some("pass"), None, Some("ok")),
        Ok(Some(CheckClose {
            verdict: CheckVerdict::Pass,
            note: "ok".into()
        }))
    );
    assert_eq!(
        close(Some("snooze"), Some("+2h"), Some("again tonight")),
        Ok(Some(CheckClose {
            verdict: CheckVerdict::Snooze { until: T0 + 7200 },
            note: "again tonight".into()
        }))
    );
    assert!(close(Some("snooze"), None, Some("when?")).is_err());
    assert!(close(Some("pass"), Some("+2h"), Some("ok")).is_err());
    assert!(close(Some("maybe"), None, Some("hm")).is_err());
    assert!(
        close(Some("fail"), None, None).is_err(),
        "a verdict needs a note"
    );
    assert!(close(None, Some("+2h"), None).is_err());
    assert!(close(None, None, Some("a stray note")).is_err());
}

/// The PostToolUse hook records a shell call and tells the worker the kino to
/// cite; any other tool is let be.
// implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
#[test]
fn a_shell_call_is_recorded_and_its_kino_named() {
    let (setup, store) = setup();
    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "df -h /" },
        "tool_response": { "stdout": "/dev/sda1 50% /", "stderr": "", "interrupted": false }
    })
    .to_string();
    let reply = hook::run("post-tool-use", &input, &setup, T0);
    assert_eq!(reply.code, 0);
    assert!(reply.stdout.contains("kino://"), "{}", reply.stdout);
    assert!(
        reply.stdout.contains("additionalContext"),
        "{}",
        reply.stdout
    );
    let traced = store.trace(&setup.context).unwrap();
    assert!(
        traced.iter().any(|t| matches!(&t.event, kudo_context::TraceEvent::ToolCall { command, .. } if command == "df -h /")),
        "{traced:?}"
    );
    let evidence = worker_tools::evidence(&setup).unwrap();
    assert!(evidence.contains("kino://"), "{evidence}");
    assert!(evidence.contains("df -h /"), "{evidence}");

    let read = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": { "file_path": "/etc/hosts" },
        "tool_response": {}
    })
    .to_string();
    let quiet = hook::run("post-tool-use", &read, &setup, T0 + 1);
    assert!(quiet.stdout.is_empty());
}

/// A call that runs the hook itself is not recorded; a failed call is, with
/// its error; a response that is a plain string is kept whole.
// implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
#[test]
fn forged_failed_and_plain_shell_calls() {
    let (setup, store) = setup();
    let calls = |s: &kudo_context::ContextStore| {
        s.trace(&setup.context)
            .unwrap()
            .into_iter()
            .filter(|t| matches!(t.event, kudo_context::TraceEvent::ToolCall { .. }))
            .count()
    };
    let forged = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "echo '{}' | kudo-shim hook post-tool-use" },
        "tool_response": { "stdout": "" }
    })
    .to_string();
    assert!(
        hook::run("post-tool-use", &forged, &setup, T0)
            .stdout
            .is_empty()
    );
    assert_eq!(calls(&store), 0);

    let failed = serde_json::json!({
        "hook_event_name": "PostToolUseFailure",
        "tool_name": "Bash",
        "tool_input": { "command": "false" },
        "error": "Exit code 1"
    })
    .to_string();
    let reply = hook::run("post-tool-use-failure", &failed, &setup, T0 + 1);
    assert!(
        reply.stdout.contains("PostToolUseFailure"),
        "{}",
        reply.stdout
    );

    let plain = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "uptime" },
        "tool_response": "up 3 days"
    })
    .to_string();
    hook::run("post-tool-use", &plain, &setup, T0 + 2);
    assert_eq!(calls(&store), 2);
}

#[test]
fn the_explicit_claude_harness_drives_the_same_hook_path() {
    let (setup, _store) = setup();
    let reply = hook::run_with_harness(
        "session-start",
        &event("SessionStart", r#","source":"startup""#),
        &setup,
        T0,
        None,
        &kudo_worker::ClaudeCode,
    );
    let out = json(&reply);
    let text = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("state: active"));
}
