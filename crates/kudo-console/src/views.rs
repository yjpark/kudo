//! The console server's reads: plain functions over the context store, so
//! everything a pane shows is computed here and tested without a browser.
//!
//! implements: d7562b1ac2f15d346f0a98561dc3da3351fabcd807f4ce4bbcd614f2f2a73c1f@9ac5369cd207a2da44b4ac1698c0b6725ef7ab4d94798e3411253a0c60d32c29

use kudo_context::{ContextStore, Error, Now, TraceEvent};

use crate::model::{ConsoleData, ContextCard, ContextDetail, PaneRef, TraceRow, WorkerView};

/// A lease silent for longer than a claim lives is likely dead (killed,
/// crashed, its machine asleep): shown as quiet, never as working.
const QUIET_AFTER: i64 = kudo_worker::DEFAULT_CLAIM_TTL;
/// A pane that never traced: quiet after this (stuck at a prompt, or dead)…
// implements: 0c4e36f264b2b4fbc6fcfee93bc02ad5b8419a1d21a8638bf080623ab942a37d@0c4e36f264b2b4fbc6fcfee93bc02ad5b8419a1d21a8638bf080623ab942a37d
const STARTING_QUIET_AFTER: i64 = 600;
/// …and no longer listed after this.
const STARTING_GONE_AFTER: i64 = 86_400;

/// Every context, newest first, with its counts and the leases still running
/// under it (a lease whose last traced event is not its end).
pub fn contexts(store: &ContextStore, now: Now) -> Result<Vec<ContextCard>, Error> {
    store
        .contexts()?
        .into_iter()
        .map(|c| {
            let mut last: Vec<(String, TraceEvent, String)> = Vec::new();
            // A lease that ended stays ended, whatever it traces after (a
            // job a human-driven session ran, say).
            let mut ended: Vec<String> = Vec::new();
            for entry in store.trace(&c.id)? {
                if matches!(entry.event, TraceEvent::LeaseEnd(_)) {
                    ended.push(entry.lease.clone());
                }
                match last.iter_mut().find(|(lease, _, _)| *lease == entry.lease) {
                    Some(slot) => {
                        slot.1 = entry.event;
                        slot.2 = entry.at;
                    }
                    None => last.push((entry.lease, entry.event, entry.at)),
                }
            }
            let panes = store.panes(&c.id)?;
            let mut starting: Vec<(String, bool)> = Vec::new();
            let pane_of = |lease: &str| {
                panes.iter().rfind(|p| p.lease == lease).map(|p| PaneRef {
                    node: p.node.clone(),
                    job: p.job.clone(),
                })
            };
            // A pane started but not traced yet (its CLI at a startup
            // prompt) is a worker too: starting, and reachable to take over.
            // Silent for minutes it is stuck or dead: quiet, then gone.
            for pane in &panes {
                let age = now - pane.at;
                if !last.iter().any(|(lease, _, _)| *lease == pane.lease)
                    && age < STARTING_GONE_AFTER
                {
                    starting.push((pane.lease.clone(), age >= STARTING_QUIET_AFTER));
                }
            }
            let workers = last
                .into_iter()
                .filter(|(lease, event, _)| {
                    !matches!(event, TraceEvent::LeaseEnd(_)) && !ended.contains(lease)
                })
                .map(|(lease, event, at)| WorkerView {
                    last: event.to_line(),
                    quiet: at
                        .parse::<jiff::Timestamp>()
                        .map(|t| now - t.as_second() > QUIET_AFTER)
                        .unwrap_or(false),
                    pane: pane_of(&lease),
                    lease,
                })
                .chain(starting.iter().map(|(lease, quiet)| WorkerView {
                    lease: lease.clone(),
                    last: "starting".to_owned(),
                    quiet: *quiet,
                    pane: pane_of(lease),
                }))
                .collect();
            let later = store.not_yet_due(&c.id, now)?;
            Ok(ContextCard {
                later: later.len(),
                next_due: later
                    .first()
                    .and_then(|i| i.due)
                    .and_then(|d| jiff::Timestamp::from_second(d).ok())
                    .map(|t| t.to_string()),
                ready: store.ready_count(&c.id, now)?,
                waiting: store.waiting_count(&c.id, now)?,
                submitted: store.submitted_count(&c.id)?,
                id: c.id.0,
                title: c.title,
                workers,
            })
        })
        .collect()
}

/// Contexts and the selected one (the newest when none is), never failing as
/// a whole: what could not be read is said in `error`, and the rest still
/// shows — so the *where* pane, the way to a different selection, stays.
pub fn console(store: &ContextStore, selected: Option<String>, now: Now) -> ConsoleData {
    let mut data = ConsoleData::default();
    match contexts(store, now) {
        Ok(contexts) => data.contexts = contexts,
        Err(e) => data.error = Some(e.to_string()),
    }
    if let Some(id) = selected.or_else(|| data.contexts.first().map(|c| c.id.clone())) {
        match detail(store, &id) {
            Ok(detail) => data.selected = Some(detail),
            Err(e) => data.error = Some(format!("{id}: {e}")),
        }
    }
    data
}

/// One context — by id or unique prefix — its head (the newest tip, flagged
/// when forked) and its trace, newest first.
pub fn detail(store: &ContextStore, id: &str) -> Result<ContextDetail, Error> {
    let ctx = store.resolve_context(id)?;
    let heads = store.heads(&ctx)?;
    let title = store
        .contexts()?
        .into_iter()
        .find(|c| c.id == ctx)
        .map(|c| c.title)
        .unwrap_or_default();
    let mut trace: Vec<TraceRow> = store
        .trace(&ctx)?
        .into_iter()
        .map(|t| TraceRow {
            at: t.at,
            lease: t.lease,
            seq: t.seq,
            line: t.event.to_line(),
        })
        .collect();
    trace.reverse();
    Ok(ContextDetail {
        id: ctx.0,
        title,
        head: heads.first().map(|h| h.body.clone()).unwrap_or_default(),
        forked: heads.len() > 1,
        trace,
    })
}

/// Every node's jobs waiting for a verdict, oldest first, each named by the
/// context whose worker asked (from `asked`, keyed by job id: a node may be
/// spelled `yolo` by its worker and `edger::yolo` by the hub).
// implements: kudo:daf63591a53811da5e761c847a528776758b90ee07fc55f8c16ce14789be1f44
pub fn approvals(
    pending: Vec<(String, moco_job::wire::PendingJob)>,
    asked: &std::collections::HashMap<String, String>,
) -> Vec<crate::model::ApprovalView> {
    let mut views: Vec<crate::model::ApprovalView> = pending
        .into_iter()
        .map(|(node, job)| crate::model::ApprovalView {
            context: asked.get(&job.id).cloned(),
            owner: job.scope.to_string(),
            waited_s: job.waited_ms / 1000,
            left_s: job.left_ms / 1000,
            argv: job.argv,
            cwd: job.cwd,
            job: job.id,
            node,
        })
        .collect();
    // Oldest first; equal waits keep one order, so rows never move under a
    // finger.
    views.sort_by(|a, b| {
        b.waited_s
            .cmp(&a.waited_s)
            .then_with(|| a.node.cmp(&b.node))
            .then_with(|| a.job.cmp(&b.job))
    });
    views
}

/// A span of seconds in the fewest words: `40 s`, `3 min`, `2 h`, `4 d`.
fn span(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s} s"),
        s if s < 90 * 60 => format!("{} min", s / 60),
        s if s < 48 * 3600 => format!("{} h", s / 3600),
        s => format!("{} d", s / 86_400),
    }
}

/// A job's state as one word and its tone: `ok`, `warn`, `bad` or `idle`.
fn state_tone(status: &moco_job::JobStatus) -> (String, &'static str) {
    use moco_job::JobStatus;
    let tone = match status {
        JobStatus::PendingApproval => "warn",
        JobStatus::Running | JobStatus::Done { code: 0 } => "ok",
        JobStatus::OutcomeUnknown => "idle",
        _ => "bad",
    };
    (kudo_shim::jobs::status_word(status), tone)
}

/// When a job was created, from its id (`<unix ms, 13 digits>-<pid>-<seq>`).
fn created_ms(id: &str) -> Option<u64> {
    id.split('-').next()?.parse().ok()
}

/// A node's jobs in words: waiting first, then running, then ended; newest
/// first within each.
// implements: 2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655@2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655
pub fn job_views(jobs: Vec<moco_job::wire::JobSummary>, now_ms: u64) -> Vec<crate::model::JobView> {
    use moco_job::JobStatus;
    let rank = |status: &JobStatus| match status {
        JobStatus::PendingApproval => 0,
        JobStatus::Running => 1,
        _ => 2,
    };
    let mut jobs = jobs;
    jobs.sort_by(|a, b| {
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then_with(|| created_ms(&b.id).cmp(&created_ms(&a.id)))
            .then_with(|| b.id.cmp(&a.id))
    });
    jobs.into_iter()
        .map(|job| {
            let (state, tone) = state_tone(&job.status);
            let phase = match job.status {
                JobStatus::PendingApproval => format!("waiting {}", span(job.phase_s)),
                JobStatus::Running => format!("running {}", span(job.phase_s)),
                // implements: 0119d16e256891de4a71a14418f59c33419207f49792e58001639f8cbef567a6@d7ba7c887089648434bc819ad57473bb930242c27396d150b11617388bb85da0
                JobStatus::OutcomeUnknown if !job.external => {
                    "the node restarted under it; no exit kept".to_owned()
                }
                _ => match created_ms(&job.id) {
                    Some(at) => format!("started {} ago", span(now_ms.saturating_sub(at) / 1000)),
                    None => "ended".to_owned(),
                },
            };
            crate::model::JobView {
                live: matches!(job.status, JobStatus::PendingApproval | JobStatus::Running),
                argv: crate::model::argv_text(&job.argv),
                // A waiting job has no verdict yet: the node's placeholder
                // (`no-approver`) would read as a refusal.
                verdict: if job.status == JobStatus::PendingApproval {
                    "awaiting your verdict".to_owned()
                } else {
                    kudo_shim::jobs::verdict_word(job.verdict)
                },
                cwd: job.cwd,
                tone: tone.to_owned(),
                state,
                phase,
                id: job.id,
            }
        })
        .collect()
}

/// A job's output from a tail, as text (lossily where it is not UTF-8).
pub fn job_output(
    node: &str,
    job: &str,
    reply: &moco_job::wire::TailReply,
) -> crate::model::JobOutput {
    let (state, tone) = state_tone(&reply.status);
    // A character cut by the end of this read is left for the next one,
    // rather than shown as a replacement character.
    let cut = match std::str::from_utf8(&reply.bytes) {
        Err(e) if e.error_len().is_none() => reply.bytes.len() - e.valid_up_to(),
        _ => 0,
    };
    let kept = reply
        .bytes
        .get(..reply.bytes.len() - cut)
        .unwrap_or_default();
    crate::model::JobOutput {
        node: node.to_owned(),
        job: job.to_owned(),
        text: String::from_utf8_lossy(kept).into_owned(),
        next_offset: reply.next_offset.saturating_sub(cut as u64),
        skipped: reply.skipped,
        live: matches!(
            reply.status,
            moco_job::JobStatus::PendingApproval | moco_job::JobStatus::Running
        ),
        state,
        tone: tone.to_owned(),
    }
}

/// Which context's worker started each job, by job id, from the contexts'
/// job-start events: its title and short id (titles need not be unique).
pub fn job_contexts(
    store: &ContextStore,
) -> Result<std::collections::HashMap<String, String>, Error> {
    let mut asked = std::collections::HashMap::new();
    for card in store.contexts()? {
        let name = format!(
            "{} {}",
            card.title,
            card.id.0.get(..12).unwrap_or(&card.id.0)
        );
        for entry in store.trace(&card.id)? {
            if let TraceEvent::JobStart { job, .. } = entry.event {
                asked.insert(job, name.clone());
            }
        }
    }
    Ok(asked)
}

/// The verdict a button names.
pub fn verdict_from(name: &str) -> Option<moco_job::wire::WireVerdict> {
    use moco_job::wire::WireVerdict;
    Some(match name {
        "allow-once" => WireVerdict::AllowOnce,
        "allow-always" => WireVerdict::AllowAlways,
        "deny-once" => WireVerdict::DenyOnce,
        "deny-always" => WireVerdict::DenyAlways,
        _ => return None,
    })
}
