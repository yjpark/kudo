//! `kudo-shim hook <event>`: the agent CLI's hooks, answered from the context
//! store with no hub and no node on the path — a worker survives both
//! restarting.
//!
//! The CLI runs a hook as a command, feeds it the event as JSON on stdin and
//! reads its decision from stdout. Each event maps to one worker decision:
//!
//! | event                | answer                                             |
//! |----------------------|----------------------------------------------------|
//! | `session-start`      | the head and the worker's rules as added context   |
//! | `stop`               | `{"decision":"block","reason":…}`, or nothing      |
//! | `pre-compact`        | ends the lease                                     |
//! | `session-end`        | ends the lease without handing off more work       |
//! | `user-prompt-submit` | ends the lease unless it is the launcher's kickoff or the CLI's own task notification |
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e

use kudo_context::Now;
use kudo_worker::{AgentHarness, ClaudeCode, LeaseBook, Worker, WorkerSetup, harness_from};
use serde_json::Value;

pub use kudo_worker::harness::HookReply;

/// Answer one hook event for the worker `setup` describes.
pub fn run(event: &str, stdin: &str, setup: &WorkerSetup, now: Now) -> HookReply {
    let harness = match harness_from(|key| std::env::var(key).ok()) {
        Ok(harness) => harness,
        Err(error) => return HookReply::failed(error),
    };
    run_with_harness(
        event,
        stdin,
        setup,
        now,
        crate::events::observer_from_env(),
        harness.as_ref(),
    )
}

/// [`run`], with the observer of the loop's events given and Claude's
/// protocol preserved for existing callers.
pub fn run_with(
    event: &str,
    stdin: &str,
    setup: &WorkerSetup,
    now: Now,
    observer: Option<kudo_worker::Observer>,
) -> HookReply {
    run_with_harness(event, stdin, setup, now, observer, &ClaudeCode)
}

/// [`run`], with the observer and the agent's wire protocol given.
pub fn run_with_harness(
    event: &str,
    stdin: &str,
    setup: &WorkerSetup,
    now: Now,
    observer: Option<kudo_worker::Observer>,
    harness: &dyn AgentHarness,
) -> HookReply {
    let input: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
    // A shell call's record needs no mirror, and must not wait on one.
    // implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
    if matches!(event, "post-tool-use" | "post-tool-use-failure") {
        return post_tool_use(&input, setup, now, harness);
    }
    let store = match crate::cli::open_store(
        &setup.store_dir,
        crate::cli::mirror_from_env().as_deref(),
        false,
    ) {
        Ok(store) => store,
        Err(e) => return HookReply::failed(e),
    };
    // A waiting Stop looks every few seconds: its mirror syncs at a wider
    // spacing (claims and submits still force theirs).
    let store = if event == "stop" {
        store.with_sync_spacing(std::time::Duration::from_secs(kudo_worker::env::WAIT_SYNC))
    } else {
        store
    };
    let book = LeaseBook::new(&setup.leases_dir());
    let mut worker = Worker::new(&store, &book, setup.config());
    if let Some(observer) = observer {
        worker = worker.with_observer(observer);
    }

    let result = match event {
        "session-start" => worker
            .session_start(now)
            .map(|text| harness.session_start_reply(&text)),
        // Waits for work: the clock runs on from `now`.
        "stop" => {
            let started = std::time::Instant::now();
            let clock = || now + i64::try_from(started.elapsed().as_secs()).unwrap_or(0);
            let pause =
                || std::thread::sleep(std::time::Duration::from_secs(kudo_worker::env::WAIT_POLL));
            worker.stop_waiting(clock, pause, kudo_worker::env::WAIT_WINDOW)
        }
        .map(|stop| harness.stop_reply(&stop)),
        "pre-compact" => worker.pre_compact(now).map(|()| harness.quiet_reply()),
        "session-end" => worker.session_end(now).map(|()| harness.quiet_reply()),
        "user-prompt-submit" => {
            let prompt = harness.user_prompt(&input).unwrap_or_default();
            if prompt == kudo_worker::KICKOFF {
                worker.session_start(now).and_then(|context| {
                    worker
                        .user_prompt(prompt, now)
                        .map(|()| harness.user_prompt_reply(Some(&context)))
                })
            } else {
                worker
                    .user_prompt(prompt, now)
                    .map(|()| harness.user_prompt_reply(None))
            }
        }
        other => return HookReply::failed(format!("no hook named `{other}`")),
    };
    result.unwrap_or_else(HookReply::failed)
}

pub fn pre_tool_use_with(
    input: &Value,
    setup: &WorkerSetup,
    now: Now,
    harness: &dyn AgentHarness,
    clock: impl Fn() -> Now,
    pause: impl FnMut(),
) -> HookReply {
    let _ = (input, setup, now, clock, pause);
    harness.pre_tool_reply(&kudo_worker::harness::PreToolDecision::Deny(
        "MCP approval is not implemented".into(),
    ))
}

/// A shell call just returned (or failed): record it in the lease's trace
/// and tell the worker the kino to cite. Any other tool is let be. Best
/// effort: a failed record never fails the call.
fn post_tool_use(
    input: &Value,
    setup: &WorkerSetup,
    now: Now,
    harness: &dyn AgentHarness,
) -> HookReply {
    let Some(call) = harness.shell_call(input) else {
        return harness.quiet_reply();
    };
    // A call that runs this hook itself would write a record no tool call
    // made: it is not recorded (the call that ran it is, and shows it).
    if call.command.contains("hook post-tool-use") {
        return harness.quiet_reply();
    }
    // Written locally only: a shell call must not wait on the network; the
    // next sync (the Stop hook, a submit) carries it to the mirror.
    let store = match crate::cli::open_store(&setup.store_dir, None, false) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("kudo: could not record the shell call: {e}");
            return harness.quiet_reply();
        }
    };
    match store.record_tool_call(
        &setup.context,
        &setup.lease,
        &call.command,
        call.exit,
        &call.output,
        now,
    ) {
        Ok(id) => harness.post_tool_reply(
            &call.event,
            &format!(
                "kudo recorded this shell call as kino://{id}: cite it as evidence for \
                 what it showed."
            ),
        ),
        Err(e) => {
            eprintln!("kudo: could not record the shell call: {e}");
            harness.quiet_reply()
        }
    }
}
