//! The queue a pull worker needs: enqueue, pull, submit, release, and claims
//! with deadlines.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e
//! implements: 6f2cfc2264e8f880d1300d1d79b96c597b995b36b756a03d6871a945b986f2dd@6f2cfc2264e8f880d1300d1d79b96c597b995b36b756a03d6871a945b986f2dd

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use kudo_context::{ContextId, ContextStore, Error, ItemId, Submission};

const T0: i64 = 1_792_500_000;
const TTL: i64 = 600;

/// A fresh, empty directory for one test's store.
fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kudo-context-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn store_with_context() -> (ContextStore, ContextId) {
    let store = ContextStore::init(&scratch()).unwrap();
    let ctx = store
        .create_context("first kudo", "# first kudo\nstate: active — new\n", T0)
        .unwrap();
    (store, ctx)
}

fn sub(head: &str, log: &str) -> Submission {
    Submission {
        head: head.into(),
        log: log.into(),
        ..Default::default()
    }
}

#[test]
fn a_new_context_has_its_first_head() {
    let (store, ctx) = store_with_context();
    let head = store.head(&ctx).unwrap();
    assert_eq!(head.body, "# first kudo\nstate: active — new\n");
    assert_eq!(store.head_history(&ctx).unwrap().len(), 1);
}

#[test]
fn an_empty_queue_has_nothing_to_pull() {
    let (store, ctx) = store_with_context();
    assert!(!store.has_ready(&ctx, T0).unwrap());
    assert_eq!(store.pull(&ctx, "lease-a", T0, TTL).unwrap(), None);
}

#[test]
fn items_are_pulled_oldest_first_and_submits_supersede_the_head() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "read the spec", T0).unwrap();
    store.enqueue(&ctx, "write the test", T0).unwrap();

    let first = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    assert_eq!(first.item.ask, "read the spec");
    assert_eq!(first.item.seq, 1);
    assert_eq!(first.attempt, 1);
    assert_eq!(first.deadline, T0 + TTL);
    store
        .submit(&first.id, &sub("head after 1\n", "read it"), T0 + 1)
        .unwrap();

    let second = store.pull(&ctx, "lease-a", T0 + 2, TTL).unwrap().unwrap();
    assert_eq!(second.item.ask, "write the test");
    store
        .submit(&second.id, &sub("head after 2\n", "wrote it"), T0 + 3)
        .unwrap();

    assert!(!store.has_ready(&ctx, T0 + 4).unwrap());
    assert_eq!(store.head(&ctx).unwrap().body, "head after 2\n");
    let bodies: Vec<String> = store
        .head_history(&ctx)
        .unwrap()
        .into_iter()
        .map(|h| h.body)
        .collect();
    assert_eq!(
        bodies,
        vec![
            "# first kudo\nstate: active — new\n",
            "head after 1\n",
            "head after 2\n"
        ]
    );
    let log = store.log(&ctx).unwrap();
    assert_eq!(log.len(), 2);
    assert!(log[0].starts_with("read it\n"), "{log:?}");
    assert!(log[1].starts_with("wrote it\n"), "{log:?}");
}

#[test]
fn identical_asks_are_distinct_items() {
    let (store, ctx) = store_with_context();
    let a = store.enqueue(&ctx, "again", T0).unwrap();
    let b = store.enqueue(&ctx, "again", T0).unwrap();
    assert_ne!(a, b);
}

#[test]
fn a_live_claim_is_exclusive() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "only one", T0).unwrap();
    let held = store.pull(&ctx, "lease-a", T0, TTL).unwrap();
    assert!(held.is_some());
    assert!(!store.has_ready(&ctx, T0 + 1).unwrap());
    assert_eq!(store.pull(&ctx, "lease-b", T0 + 1, TTL).unwrap(), None);
}

#[test]
fn an_expired_claim_is_pulled_again_as_the_next_attempt() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "crashes", T0).unwrap();
    let first = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();

    assert!(store.has_ready(&ctx, T0 + TTL + 1).unwrap());
    let retry = store
        .pull(&ctx, "lease-b", T0 + TTL + 1, TTL)
        .unwrap()
        .unwrap();
    assert_eq!(retry.item, first.item);
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.lease, "lease-b");

    // The first worker comes back too late: its submit is refused.
    let late = store.submit(&first.id, &sub("stale\n", "late"), T0 + TTL + 2);
    assert!(matches!(late, Err(Error::ClaimExpired(_))), "{late:?}");
}

#[test]
fn a_released_claim_is_ready_again_at_once() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "give back", T0).unwrap();
    let first = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    store.release(&first.id, T0 + 1).unwrap();

    let again = store.pull(&ctx, "lease-a", T0 + 2, TTL).unwrap().unwrap();
    assert_eq!(again.item, first.item);
    assert_eq!(again.attempt, 2);
}

#[test]
fn a_settled_claim_cannot_be_submitted_or_released_again() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "once", T0).unwrap();
    let claim = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    store
        .submit(&claim.id, &sub("done\n", "did it"), T0 + 1)
        .unwrap();

    let twice = store.submit(&claim.id, &sub("again\n", "again"), T0 + 2);
    assert!(matches!(twice, Err(Error::ClaimSettled(_))), "{twice:?}");
    let release = store.release(&claim.id, T0 + 2);
    assert!(
        matches!(release, Err(Error::ClaimSettled(_))),
        "{release:?}"
    );
}

#[test]
fn a_reopened_store_sees_everything_written_before() {
    let dir = scratch();
    let ctx = {
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("t", "h0\n", T0).unwrap();
        store.enqueue(&ctx, "survives", T0).unwrap();
        ctx
    };
    let store = ContextStore::open(&dir).unwrap();
    let claim = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    assert_eq!(claim.item.ask, "survives");
}

#[test]
fn queues_of_two_contexts_do_not_mix() {
    let (store, a) = store_with_context();
    let b = store.create_context("other", "other head\n", T0).unwrap();
    store.enqueue(&a, "for a", T0).unwrap();
    assert_eq!(store.pull(&b, "lease-b", T0, TTL).unwrap(), None);
    assert_eq!(
        store
            .pull(&a, "lease-a", T0, TTL)
            .unwrap()
            .unwrap()
            .item
            .ask,
        "for a"
    );
}

#[test]
fn a_lease_knows_the_claim_it_holds_until_it_is_settled_or_expires() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "held", T0).unwrap();
    assert_eq!(store.held_by(&ctx, "lease-a", T0).unwrap(), None);

    let claim = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    assert_eq!(
        store.held_by(&ctx, "lease-a", T0 + 1).unwrap(),
        Some(claim.clone())
    );
    assert_eq!(store.held_by(&ctx, "lease-b", T0 + 1).unwrap(), None);
    assert_eq!(store.held_by(&ctx, "lease-a", T0 + TTL + 1).unwrap(), None);

    store
        .submit(&claim.id, &sub("h\n", "done"), T0 + 2)
        .unwrap();
    assert_eq!(store.held_by(&ctx, "lease-a", T0 + 3).unwrap(), None);
}

#[test]
fn the_ready_count_drops_as_items_are_claimed_and_submitted() {
    let (store, ctx) = store_with_context();
    store.enqueue(&ctx, "a", T0).unwrap();
    store.enqueue(&ctx, "b", T0).unwrap();
    assert_eq!(store.ready_count(&ctx, T0).unwrap(), 2);
    let claim = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    assert_eq!(store.ready_count(&ctx, T0).unwrap(), 1);
    store.submit(&claim.id, &sub("h\n", "l"), T0 + 1).unwrap();
    assert_eq!(store.ready_count(&ctx, T0 + 1).unwrap(), 1);
}

// implements: a99975ca76ea8d354544294d35af3d49c1215a2029be4aa91f51e541fc26fdc7
#[test]
fn an_item_waits_for_every_dependency_to_submit() {
    let (store, ctx) = store_with_context();
    let first = store.enqueue(&ctx, "first", T0).unwrap();
    let second = store
        .enqueue_with_dependencies(&ctx, "second", None, &[first.clone()], T0)
        .unwrap();
    let third = store
        .enqueue_with_dependencies(&ctx, "third", None, &[second.clone()], T0)
        .unwrap();

    assert_eq!(store.ready_count(&ctx, T0).unwrap(), 1);
    let claim = store.pull(&ctx, "lease-a", T0, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, first);
    store.submit(&claim.id, &sub("h1\n", "one"), T0 + 1).unwrap();

    let claim = store.pull(&ctx, "lease-a", T0 + 2, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, second);
    store.release(&claim.id, T0 + 3).unwrap();
    let claim = store.pull(&ctx, "lease-a", T0 + 4, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, second, "release does not satisfy it");
    store.submit(&claim.id, &sub("h2\n", "two"), T0 + 5).unwrap();

    let claim = store.pull(&ctx, "lease-a", T0 + 6, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, third);
}

// implements: a99975ca76ea8d354544294d35af3d49c1215a2029be4aa91f51e541fc26fdc7
#[test]
fn dependencies_must_be_distinct_existing_items_in_the_same_context() {
    let (store, ctx) = store_with_context();
    let first = store.enqueue(&ctx, "first", T0).unwrap();
    let other = store.create_context("other", "other\n", T0).unwrap();
    let foreign = store.enqueue(&other, "foreign", T0).unwrap();
    let missing = ItemId("ab".repeat(32));

    for deps in [
        vec![missing],
        vec![foreign],
        vec![first.clone(), first.clone()],
    ] {
        let result = store.enqueue_with_dependencies(&ctx, "bad", None, &deps, T0);
        assert!(matches!(result, Err(Error::InvalidDependency(_))), "{result:?}");
    }
}

// implements: 8c1fc5e54c5e48bb4086bd6b27a2d09bf41e100de7c743c6b04fd44b48af3d54
#[test]
fn a_blocked_submit_records_the_attempt_without_finishing_or_moving_the_head() {
    let (store, ctx) = store_with_context();
    let original_head = store.head(&ctx).unwrap();
    let item = store.enqueue(&ctx, "dependent work", T0).unwrap();
    let blocker = store.enqueue(&ctx, "prerequisite", T0 + 1).unwrap();
    let claim = store.pull(&ctx, "lease-a", T0 + 2, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, item);

    let mut blocked = sub("must not land\n", "waiting for the prerequisite");
    blocked.blocked_on = vec![blocker.clone()];
    store.submit(&claim.id, &blocked, T0 + 3).unwrap();

    assert_eq!(store.head(&ctx).unwrap(), original_head);
    assert_eq!(store.log(&ctx).unwrap().len(), 1);
    assert!(store.log(&ctx).unwrap()[0].contains("waiting for the prerequisite"));
    assert_eq!(store.ready_count(&ctx, T0 + 4).unwrap(), 1);

    let claim = store.pull(&ctx, "lease-b", T0 + 4, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, blocker, "the blocked item stays waiting");
    store.submit(&claim.id, &sub("prerequisite done\n", "done"), T0 + 5).unwrap();

    let claim = store.pull(&ctx, "lease-a", T0 + 6, TTL).unwrap().unwrap();
    assert_eq!(claim.item.id, item);
    assert_eq!(claim.attempt, 2);
    store.submit(&claim.id, &sub("finished\n", "finished"), T0 + 7).unwrap();
    assert_eq!(store.head(&ctx).unwrap().body, "finished\n");
}
