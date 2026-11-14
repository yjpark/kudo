//! What the console shows, read from the store: contexts with their live
//! workers, and the selected context's head and trace.
//!
//! implements: d7562b1ac2f15d346f0a98561dc3da3351fabcd807f4ce4bbcd614f2f2a73c1f@9ac5369cd207a2da44b4ac1698c0b6725ef7ab4d94798e3411253a0c60d32c29

#![cfg(feature = "server")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use kudo_console::views;
use kudo_context::{ClaimId, ContextStore, Submission, TraceEvent};

const T0: i64 = 1_792_500_000;

fn store(name: &str) -> ContextStore {
    let dir = std::env::temp_dir().join(format!("kudo-console-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    ContextStore::init(&dir).unwrap()
}

#[test]
fn a_card_shows_counts_and_the_leases_still_running() {
    let store = store("cards");
    let ctx = store.create_context("carded", "# carded\n", T0).unwrap();
    store.enqueue(&ctx, "one", T0).unwrap();
    store
        .record_trace(&ctx, "lease-live", &TraceEvent::LeaseStart, T0)
        .unwrap();
    store
        .record_trace(
            &ctx,
            "lease-live",
            &TraceEvent::Pull(ClaimId("c1".into())),
            T0 + 1,
        )
        .unwrap();
    store
        .record_trace(&ctx, "lease-done", &TraceEvent::LeaseStart, T0)
        .unwrap();
    store
        .record_trace(
            &ctx,
            "lease-done",
            &TraceEvent::LeaseEnd("queue-empty".into()),
            T0 + 2,
        )
        .unwrap();

    let cards = views::contexts(&store, T0 + 3).unwrap();
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(card.title, "carded");
    assert_eq!(card.ready, 1);
    let leases: Vec<&str> = card.workers.iter().map(|w| w.lease.as_str()).collect();
    assert_eq!(leases, ["lease-live"], "an ended lease is not a worker");
    assert_eq!(card.workers[0].last, "pull c1");
}

// implements: 8c1fc5e54c5e48bb4086bd6b27a2d09bf41e100de7c743c6b04fd44b48af3d54
#[test]
fn a_blocked_attempt_is_waiting_not_submitted() {
    let store = store("blocked");
    let ctx = store.create_context("blocked", "# b\n", T0).unwrap();
    store.enqueue(&ctx, "dependent", T0).unwrap();
    let blocker = store.enqueue(&ctx, "blocker", T0).unwrap();
    let claim = store.pull(&ctx, "lease", T0, 600).unwrap().unwrap();
    store
        .submit(
            &claim.id,
            &Submission {
                head: "not landed\n".into(),
                log: "waiting".into(),
                blocked_on: vec![blocker],
                ..Default::default()
            },
            T0 + 1,
        )
        .unwrap();

    let card = &views::contexts(&store, T0 + 2).unwrap()[0];
    assert_eq!(card.ready, 1, "only the blocker is ready");
    assert_eq!(card.waiting, 1);
    assert_eq!(card.submitted, 0);
}

#[test]
fn the_detail_is_the_head_and_the_trace_newest_first() {
    let store = store("detail");
    let ctx = store
        .create_context("detailed", "# detailed\nstate: new\n", T0)
        .unwrap();
    store
        .record_trace(&ctx, "l", &TraceEvent::LeaseStart, T0)
        .unwrap();
    store
        .record_trace(&ctx, "l", &TraceEvent::LeaseEnd("budget".into()), T0 + 5)
        .unwrap();
    let detail = views::detail(&store, &ctx.0).unwrap();
    assert_eq!(detail.title, "detailed");
    assert!(detail.head.contains("state: new"));
    assert!(!detail.forked);
    let lines: Vec<&str> = detail.trace.iter().map(|t| t.line.as_str()).collect();
    assert_eq!(lines, ["lease-end budget", "lease-start"]);
}

/// A lease that died without its end (killed, crashed, slept) shows as quiet
/// once it has said nothing for longer than a claim lives — never as working.
#[test]
fn a_lease_silent_past_a_claims_life_is_quiet() {
    let store = store("quiet");
    let ctx = store.create_context("quiet", "# q\n", T0).unwrap();
    store
        .record_trace(&ctx, "lease-gone", &TraceEvent::LeaseStart, T0)
        .unwrap();
    let fresh = views::contexts(&store, T0 + 60).unwrap();
    assert!(!fresh[0].workers[0].quiet);
    let later = views::contexts(&store, T0 + kudo_worker::DEFAULT_CLAIM_TTL + 1).unwrap();
    assert!(later[0].workers[0].quiet, "{later:?}");
}

/// A selection that does not resolve leaves the console usable: the contexts
/// are still listed, and the error is said beside them.
#[test]
fn a_selection_that_does_not_resolve_still_lists_the_contexts() {
    let store = store("badsel");
    store.create_context("still here", "# s\n", T0).unwrap();
    let data = views::console(&store, Some("ffffffffffff".into()), T0);
    assert_eq!(data.contexts.len(), 1, "{data:?}");
    assert!(data.selected.is_none());
    assert!(
        data.error
            .as_deref()
            .is_some_and(|e| e.contains("ffffffffffff")),
        "{data:?}"
    );
}

/// A worker the console started carries its pane: the node and job to show
/// and type into.
#[test]
fn a_worker_started_in_a_pane_carries_it() {
    let store = store("paned");
    let ctx = store.create_context("paned", "# p\n", T0).unwrap();
    store
        .record_trace(&ctx, "lease-p", &TraceEvent::LeaseStart, T0)
        .unwrap();
    store
        .record_pane(&ctx, "lease-p", "yolo", "job-3", T0)
        .unwrap();
    let cards = views::contexts(&store, T0 + 1).unwrap();
    let pane = cards[0].workers[0].pane.clone().expect("a pane");
    assert_eq!(pane.node, "yolo");
    assert_eq!(pane.job, "job-3");
}

/// A pane started but not yet traced (its CLI is at a startup prompt) is a
/// worker too — starting — so it can be taken over.
#[test]
fn a_pane_with_no_trace_yet_is_a_starting_worker() {
    let store = store("starting");
    let ctx = store.create_context("starting", "# s\n", T0).unwrap();
    store
        .record_pane(&ctx, "lease-new", "yolo", "job-9", T0)
        .unwrap();
    let cards = views::contexts(&store, T0 + 1).unwrap();
    let w = &cards[0].workers[0];
    assert_eq!(w.lease, "lease-new");
    assert_eq!(w.last, "starting");
    assert_eq!(w.pane.as_ref().unwrap().job, "job-9");
}

/// A pane that never traced goes quiet after ten minutes (still offered for
/// take over) and leaves the listing after a day.
// implements: 0c4e36f264b2b4fbc6fcfee93bc02ad5b8419a1d21a8638bf080623ab942a37d@0c4e36f264b2b4fbc6fcfee93bc02ad5b8419a1d21a8638bf080623ab942a37d
#[test]
fn a_pane_that_never_traced_goes_quiet_then_leaves() {
    let store = store("stale");
    let ctx = store.create_context("stale", "# s\n", T0).unwrap();
    store
        .record_pane(&ctx, "lease-stuck", "yolo", "job-1", T0)
        .unwrap();
    let worker = |now| views::contexts(&store, now).unwrap()[0].workers.clone();
    assert!(!worker(T0 + 599)[0].quiet, "starting, not yet quiet");
    let late = worker(T0 + 600);
    assert!(late[0].quiet && late[0].pane.is_some(), "{late:?}");
    assert!(
        worker(T0 + 86_400).is_empty(),
        "a day-old dead pane is gone"
    );
}

/// A lease that ended stays ended in the console, whatever it traces after —
/// a job its human-driven session ran, say.
#[test]
fn an_ended_lease_stays_ended() {
    let store = store("ended-stays");
    let ctx = store.create_context("ended", "# ended\n", T0).unwrap();
    for (n, event) in [
        TraceEvent::LeaseStart,
        TraceEvent::LeaseEnd("takeover".into()),
        TraceEvent::JobStart {
            node: "pc".into(),
            job: "j".into(),
            waiting: false,
            argv: "uptime".into(),
        },
    ]
    .iter()
    .enumerate()
    {
        store
            .record_trace(&ctx, "lease-x", event, T0 + n as i64)
            .unwrap();
    }
    let cards = views::contexts(&store, T0 + 10).unwrap();
    assert!(cards[0].workers.is_empty(), "{:?}", cards[0].workers);
}

/// A card counts the items not yet due and names the next one's time.
// implements: 716432a845eb22babf957938aaa9a9c0f3cd3bea3ebee24c89a5147a795e7e2b
#[test]
fn a_card_counts_what_is_not_yet_due() {
    let store = store("not-yet-due");
    let ctx = store.create_context("lab", "# lab\n", T0).unwrap();
    store
        .enqueue_due(&ctx, "later", Some(T0 + 3600), T0)
        .unwrap();
    store
        .enqueue_due(&ctx, "sooner", Some(T0 + 600), T0)
        .unwrap();
    let card = views::contexts(&store, T0).unwrap().remove(0);
    assert_eq!(card.ready, 0);
    assert_eq!(card.later, 2);
    assert_eq!(card.next_due.as_deref(), Some("2026-10-20T12:50:00Z"));
}
