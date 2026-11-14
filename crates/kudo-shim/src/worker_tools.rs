//! The worker's MCP tools — `context_read`, `evidence`, `pull`, `submit`, `release` — as
//! plain functions over the context store. The server wraps them; like the
//! hooks, they reach the store directly, never through the hub.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e

use kudo_context::{ClaimId, ContextStore, Error, Now, Submission};
use kudo_worker::{LeaseBook, Pull, Worker, WorkerSetup};

fn open(setup: &WorkerSetup) -> Result<ContextStore, String> {
    crate::cli::open_store(
        &setup.store_dir,
        crate::cli::mirror_from_env().as_deref(),
        false,
    )
}

/// The worker for these tools, telling the hub of the loop's events when one
/// is configured.
fn worker<'a>(store: &'a ContextStore, book: &'a LeaseBook, setup: &WorkerSetup) -> Worker<'a> {
    let worker = Worker::new(store, book, setup.config());
    match crate::events::observer_from_env() {
        Some(observer) => worker.with_observer(observer),
        None => worker,
    }
}

/// The bound context's head.
pub fn context_read(setup: &WorkerSetup) -> Result<String, String> {
    let heads = open(setup)?
        .heads(&setup.context)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Context {}. {}",
        setup.context.0,
        kudo_worker::render_heads(&heads)
    ))
}

/// The shell evidence this lease has recorded, with the ids to cite.
pub fn evidence(setup: &WorkerSetup) -> Result<String, String> {
    let records = open(setup)?
        .tool_evidence(&setup.context, &setup.lease)
        .map_err(|error| error.to_string())?;
    if records.is_empty() {
        return Ok("No shell evidence recorded for this lease yet.".to_owned());
    }
    let mut out = "Shell evidence for this lease; cite the kino ids in submit:".to_owned();
    for record in records {
        let exit = record
            .exit
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
        out.push_str(&format!(
            "\n- kino://{} — {} (exit {exit})",
            record.id, record.command
        ));
    }
    Ok(out)
}

/// Claim the next item under the bound context for this lease — unless this
/// turn already submitted one, when the answer is to end the turn.
pub fn pull(setup: &WorkerSetup, now: Now) -> Result<String, String> {
    let store = open(setup)?;
    let book = LeaseBook::new(&setup.leases_dir());
    let worker = worker(&store, &book, setup);
    let pulled = worker.pull(now).map_err(|e| e.to_string())?;
    Ok(match pulled {
        Pull::Claimed(c) => format!(
            "Claimed, live for {}s. {}",
            setup.claim_ttl,
            worker.claim_text(&c).map_err(|e| e.to_string())?
        ),
        Pull::Nothing => format!("Nothing ready under context {}.", setup.context.0),
        Pull::EndTurn => "You already submitted an item this turn: end your turn now. If \
                          another item is ready it is handed to you as the next turn."
            .to_owned(),
    })
}

/// Submit the work for a claim: the typed submission, with the full new head.
/// The reply names any established line that landed as claimed.
// implements: 4e67251bc978f34e7d6ce4278eb1503f9e803e51f0057d4d5954c701830f22b9@1b7ca6a98f08e1312c89a21ba22a046c2f9677965e58dd2fde0cae2756307a74
pub fn submit(
    setup: &WorkerSetup,
    claim: &str,
    submission: &Submission,
    now: Now,
) -> Result<String, String> {
    let store = open(setup)?;
    let book = LeaseBook::new(&setup.leases_dir());
    let done =
        match worker(&store, &book, setup).submit(&ClaimId(claim.to_owned()), submission, now) {
            Ok(done) => done,
            Err(Error::HeadMoved { current, .. }) => return Err(moved(&store, claim, &current)),
            Err(e) => return Err(e.to_string()),
        };
    let mut reply = if submission.blocked_on.is_empty() {
        format!(
            "Submitted {} for claim {claim}; the head is superseded.",
            done.id.0
        )
    } else {
        format!(
            "Blocked {} for claim {claim}; the head is unchanged and the item waits for {}.",
            done.id.0,
            submission
                .blocked_on
                .iter()
                .map(|id| id.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if !done.downgraded.is_empty() {
        reply.push_str(
            "\n\nThese established lines landed as claimed — none of their evidence \
             resolved. Evidence is an http(s) URL, a kino in the ledger of a repo the \
             context declares, or an earlier submission's id; the queue's own ids \
             (claims, items, the context, heads) back nothing:",
        );
        for line in &done.downgraded {
            reply.push_str(&format!("\n- {line}"));
        }
    }
    if !done.unresolved.is_empty() {
        reply.push_str(&format!(
            "\n\nEvidence that did not resolve: {}.",
            done.unresolved.join(", ")
        ));
    }
    reply.push_str("\n\nEnd your turn now: the next item, if any, is handed to you when you stop.");
    Ok(reply)
}

/// The refusal of a submit from a moved head: the head as it is now, and how
/// to build on it.
// implements: 3a472b4021f7b7496dd37779f3d0c76cafe1c95e8e80b1b236786a81e4c7d0ab@aaddfc2e7ee067ea5780ce6ee2dfe4587bfb47b0c067b2670a31308c019a7e21
fn moved(store: &ContextStore, claim: &str, current: &[String]) -> String {
    let heads = match current
        .iter()
        .map(|v| store.head_version(v))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(heads) => heads,
        Err(e) => return e.to_string(),
    };
    let mut out = format!(
        "Not submitted: another worker superseded the head since claim {claim} was \
         taken. Your claim is still yours. The head as it is now:\n"
    );
    for head in &heads {
        out.push_str(&format!(
            "\n--- version {}\n{}\n",
            head.version,
            head.body.trim_end()
        ));
    }
    out.push_str(&format!(
        "\nFold what it established into your own new head, then call `submit` \
         again with the same claim and base \"{}\".",
        current.join(",")
    ));
    out
}

/// Give a claim back unfinished.
pub fn release(setup: &WorkerSetup, claim: &str, now: Now) -> Result<String, String> {
    let store = open(setup)?;
    let book = LeaseBook::new(&setup.leases_dir());
    worker(&store, &book, setup)
        .release(&ClaimId(claim.to_owned()), now)
        .map_err(|e| e.to_string())?;
    Ok(format!("Released claim {claim}; its item is ready again."))
}

/// File a rough edge in the context's inbox: one line, optionally a body.
pub fn inbox(
    setup: &WorkerSetup,
    line: &str,
    body: Option<&str>,
    now: Now,
) -> Result<String, String> {
    let id = open(setup)?
        .file_inbox(&setup.context, &setup.lease, line, body, now)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "filed {id} in the inbox of context {}",
        setup.context.0
    ))
}

/// Trace a job this worker started: where, its id there, whether it waits
/// for the author's verdict, and what it asked to run.
// implements: kudo:e23a8b06aa9331b3f1ffd8df36be1392daf44e93ca20037b50cec6c3132f53e1@d9656cc142296b3c9017b9d73ca59d326c9a29abd2945a5afd4a73eb6514f593
pub fn trace_job_start(
    setup: &WorkerSetup,
    node: &str,
    job: &str,
    waiting: bool,
    argv: &[String],
    now: Now,
) -> Result<String, String> {
    let store = open(setup)?;
    let event = kudo_context::TraceEvent::JobStart {
        node: node.to_owned(),
        job: job.to_owned(),
        waiting,
        argv: argv.join(" "),
    };
    let id = store
        .record_trace(&setup.context, &setup.lease, &event, now)
        .map_err(|e| e.to_string())?;
    publish(setup, &event);
    Ok(id)
}

/// Say a traced job event on the hub, as the loop's own events are, so the
/// console moves at once. Best effort.
fn publish(setup: &WorkerSetup, event: &kudo_context::TraceEvent) {
    if let Some(observe) = crate::events::observer_from_env() {
        observe(&setup.context, &setup.lease, event);
    }
}

/// Whether this lease started `job` and has not traced its end: the only
/// jobs whose end is this worker's to trace. A job id is unique on its node,
/// and the node may be spelled more than one way (`yolo`, `edger::yolo`), so
/// the id alone decides.
pub fn awaits_end(setup: &WorkerSetup, job: &str) -> Result<bool, String> {
    use kudo_context::TraceEvent;
    let store = open(setup)?;
    let mut started = false;
    for t in store.trace(&setup.context).map_err(|e| e.to_string())? {
        if t.lease != setup.lease {
            continue;
        }
        match t.event {
            TraceEvent::JobStart { job: j, .. } if j == job => started = true,
            TraceEvent::JobEnd { job: j, .. } if j == job => return Ok(false),
            _ => {}
        }
    }
    Ok(started)
}

/// A job's output as a record keeps it: text (lossily where it is not UTF-8)
/// without control bytes other than newlines and tabs, and a first line
/// saying how much the node had already dropped, when it had.
pub fn output_text(bytes: &[u8], skipped: u64) -> String {
    let text: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    if skipped > 0 {
        format!("[{skipped} earlier bytes were dropped by the node]\n{text}")
    } else {
        text
    }
}

/// Trace the end of a job this worker started, once, with what it printed:
/// the record's kino id, or `None` when the job is not one this lease
/// started, or its end is already traced (matched by the job id; see
/// [`awaits_end`]).
// implements: kudo:e23a8b06aa9331b3f1ffd8df36be1392daf44e93ca20037b50cec6c3132f53e1@d9656cc142296b3c9017b9d73ca59d326c9a29abd2945a5afd4a73eb6514f593
pub fn trace_job_end(
    setup: &WorkerSetup,
    node: &str,
    job: &str,
    status: &str,
    verdict: &str,
    output: &str,
    now: Now,
) -> Result<Option<String>, String> {
    use kudo_context::TraceEvent;
    if !awaits_end(setup, job)? {
        return Ok(None);
    }
    let store = open(setup)?;
    let event = TraceEvent::JobEnd {
        node: node.to_owned(),
        job: job.to_owned(),
        status: status.to_owned(),
        verdict: verdict.to_owned(),
    };
    let id = store
        .record_job_end(&setup.context, &setup.lease, &event, output, now)
        .map_err(|e| e.to_string())?;
    publish(setup, &event);
    Ok(Some(id))
}

/// A check's verdict from the `submit` tool's arguments: `pass`, `fail`, or
/// `snooze` with when to look again (`until`), always with a note; `None`
/// when no verdict was given. Anything else is refused, saying how to fix it.
// implements: c11043084f97c61ff3f41e4d07fb2361aa871213021e9c15d0e4cee37c99bde1
pub fn check_close(
    verdict: Option<&str>,
    until: Option<&str>,
    note: Option<&str>,
    now: Now,
) -> Result<Option<kudo_context::CheckClose>, String> {
    use kudo_context::{CheckClose, CheckVerdict};
    let Some(verdict) = verdict else {
        return match (until, note) {
            (Some(_), _) => Err("snooze_until goes with verdict `snooze`".into()),
            (None, Some(_)) => Err("note goes with a check's verdict".into()),
            (None, None) => Ok(None),
        };
    };
    let note = note
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or("a verdict needs a note: say what you saw")?;
    let verdict = match (verdict, until) {
        ("pass", None) => CheckVerdict::Pass,
        ("fail", None) => CheckVerdict::Fail,
        ("snooze", Some(until)) => CheckVerdict::Snooze {
            until: crate::cli::parse_when(until, now)?,
        },
        ("snooze", None) => {
            return Err("a snooze needs snooze_until: when to look again".into());
        }
        ("pass" | "fail", Some(_)) => {
            return Err("snooze_until goes with verdict `snooze` only".into());
        }
        (other, _) => {
            return Err(format!(
                "{other:?} is not a verdict: use pass, fail or snooze"
            ));
        }
    };
    Ok(Some(CheckClose {
        verdict,
        note: note.to_owned(),
    }))
}
