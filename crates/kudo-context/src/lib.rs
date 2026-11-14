//! kudo-context — the context engine.
//!
//! A **context** is the unit of memory a worker is bound to: a superseded head
//! (where / next / established / open), its log of submissions, the repos it
//! spans, and its queue of items
//! (`kudo:6f2cfc2264e8f880d1300d1d79b96c597b995b36b756a03d6871a945b986f2dd`).
//! Its store must be reachable with no node online
//! (`kudo:4909f3e9f6d391d8a20dc8587a1ffc33160eb238d86d1cc55d5b9fb9ce754e76`).
//!
//! This crate is an engine, not a service: every composition root that needs
//! contexts links it and talks to the store directly.
//!
//! Phase 1 gives it the queue a pull worker needs: enqueue, pull (a claim with a
//! deadline), submit (supersedes the head, settles the claim) and release. Every
//! write is a kino in a local kinora store; nothing is edited in place.
//!
//! task: 4aef99814b668f7cbb8900eee43e35b67fdb6dbcd18ba196858f6226874cc0e2@715de6400c53b648fca5c4b6005b5b0be2327af9d505061c29fd17377f6f565a

pub mod error;
pub mod store;

pub use error::Error;
pub use kinora_remote::Mirror;
pub use store::ContextStore;

/// Seconds since the Unix epoch. Every time-dependent call takes `now`
/// explicitly, so deadlines are testable without a clock.
pub type Now = i64;

/// A context's stable identity (its kinora id).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextId(pub String);

/// A queue item's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId(pub String);

/// A claim's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaimId(pub String);

/// A submission's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubmissionId(pub String);

/// The context's head: the current version of its resume block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The version hash of this head.
    pub version: String,
    /// The resume block as markdown.
    pub body: String,
}

/// One item of work enqueued under a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    /// Enqueue order within its context, from 1.
    pub seq: u64,
    /// The ask, as typed (a check's: what to look at).
    pub ask: String,
    /// When it becomes ready; `None` for ready at once.
    pub due: Option<Now>,
    /// Earlier items that must submit before this one is ready.
    pub after: Vec<ItemId>,
    /// The check it is, if it is one.
    pub check: Option<Check>,
    /// The check item a snooze re-queued this one from.
    pub snoozed_from: Option<ItemId>,
}

/// A check: what to look at, the measurable condition that passes it, and
/// the evidence window it is judged on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Check {
    pub what: String,
    pub pass: String,
    pub window: String,
}

/// How a check closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckVerdict {
    Pass,
    Fail,
    /// Look again at `until` (epoch seconds): the check is enqueued again,
    /// due then.
    Snooze {
        until: Now,
    },
}

/// A check's verdict and the note that goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckClose {
    pub verdict: CheckVerdict,
    pub note: String,
}

/// A worker's claim on an item: exclusive until settled or past its deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: ClaimId,
    pub context: ContextId,
    pub item: Item,
    /// 1 for the first claim on the item, N+1 after attempt N settled or expired.
    pub attempt: u32,
    /// The lease (session) that holds the claim.
    pub lease: String,
    pub deadline: Now,
    /// The head versions current when the claim was taken (one, unless the
    /// head was forked): a submit must build on them, or name what it built on.
    pub base: Vec<String>,
}

/// What a worker hands back for the item it claimed: one typed input, from
/// which the submission's body is rendered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Submission {
    /// The full new head body, as the worker leaves it.
    pub head: String,
    /// A few lines on what was done, in the worker's words.
    pub log: String,
    /// What the worker established, each with its evidence.
    pub established: Vec<Established>,
    /// What the worker only claims.
    pub claimed: Vec<String>,
    /// What the worker ruled out, and why.
    pub ruled_out: Vec<RuledOut>,
    /// What the worker could not see.
    pub gaps: Vec<String>,
    /// The head versions the new head builds on, when not the claim's own
    /// base — after a refusal, the versions it handed back.
    pub base: Option<Vec<String>>,
    /// A check's verdict: required on a check's claim, refused on any other.
    pub check: Option<CheckClose>,
    /// Items that must submit before this claim's item becomes ready again.
    pub blocked_on: Vec<ItemId>,
}

/// A rough edge filed from inside the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    pub line: String,
    pub body: Option<String>,
    /// The lease (session) that filed it.
    pub lease: String,
}

/// Choices fixed when a context is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextOptions {
    /// Give each worker lease its own worktree for a named repo.
    pub worktree: bool,
}

/// A context as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSummary {
    pub id: ContextId,
    pub title: String,
    /// When it was created, as an RFC 3339 timestamp.
    pub created: String,
}

/// One established line and its evidence: kino ids, or URLs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Established {
    pub claim: String,
    pub evidence: Vec<String>,
}

/// A candidate the worker ruled out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuledOut {
    pub candidate: String,
    pub because: String,
}

/// What a submit stored, and what it could not take as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    pub id: SubmissionId,
    /// Established lines moved to claimed: none of their evidence resolved.
    pub downgraded: Vec<String>,
    /// Evidence entries that did not resolve.
    pub unresolved: Vec<String>,
    /// The check a snooze enqueued again.
    pub requeued: Option<ItemId>,
}

/// A repo a context spans: by name, on a node, at a path there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Repo {
    pub name: String,
    /// The machine's name: its hostname, or `KUDO_NODE`.
    pub node: String,
    /// Absolute.
    pub path: std::path::PathBuf,
}

/// One event of the pull loop, as the hooks and tools record it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
    /// The session started under the context.
    LeaseStart,
    /// The Stop hook handed over an item.
    Handoff(ClaimId),
    /// The `pull` tool claimed an item.
    Pull(ClaimId),
    /// A stop was refused while the claim was open.
    Refused(ClaimId),
    /// The claim was given back.
    Release(ClaimId),
    /// The claim's work was submitted.
    Submit {
        claim: ClaimId,
        submission: SubmissionId,
    },
    /// The lease ended, and why.
    LeaseEnd(String),
    /// Nothing was ready; the lease waits for the next item.
    Wait,
    /// The worker started a job on a node: its id there (the pointer into
    /// the node's audit), whether it waits for the author's verdict, and the
    /// argv it asked for.
    JobStart {
        node: String,
        job: String,
        waiting: bool,
        argv: String,
    },
    /// A shell call the worker made, recorded as it returned: the command and
    /// its exit (the output is in the kino's body).
    ToolCall { command: String, exit: Option<i32> },
    /// The worker saw that job end: how (`done:0`, `denied:rule`, …), and
    /// under which verdict.
    JobEnd {
        node: String,
        job: String,
        status: String,
        verdict: String,
    },
}

// implements: 03045b42c8f5eb9f6b68ffe4b372e8560b43d4da21c4a77c6ddab998713b7c6d@03045b42c8f5eb9f6b68ffe4b372e8560b43d4da21c4a77c6ddab998713b7c6d
impl TraceEvent {
    /// The event's name, as topics and trace kinos spell it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::LeaseStart => "lease-start",
            Self::Handoff(_) => "handoff",
            Self::Pull(_) => "pull",
            Self::Refused(_) => "refused",
            Self::Release(_) => "release",
            Self::Submit { .. } => "submit",
            Self::LeaseEnd(_) => "lease-end",
            Self::Wait => "wait",
            Self::JobStart { .. } => "job-start",
            Self::JobEnd { .. } => "job",
            Self::ToolCall { .. } => "tool-call",
        }
    }

    /// The event as one line: its name, then what it names — the text a
    /// trace kino holds and an event payload carries.
    pub fn to_line(&self) -> String {
        match self {
            Self::LeaseStart | Self::Wait => self.name().to_owned(),
            Self::Handoff(c) | Self::Pull(c) | Self::Refused(c) | Self::Release(c) => {
                format!("{} {}", self.name(), c.0)
            }
            Self::Submit { claim, submission } => {
                format!("{} {} {}", self.name(), claim.0, submission.0)
            }
            Self::LeaseEnd(why) => format!("{} {why}", self.name()),
            Self::JobStart {
                node,
                job,
                waiting,
                argv,
            } => format!(
                "{} {node} {job} {} {argv}",
                self.name(),
                if *waiting { "waiting" } else { "running" }
            ),
            Self::JobEnd {
                node,
                job,
                status,
                verdict,
            } => format!("{} {node} {job} {status} {verdict}", self.name()),
            Self::ToolCall { command, exit } => format!(
                "{} {} {command}",
                self.name(),
                exit.map_or_else(|| "?".to_owned(), |c| c.to_string())
            ),
        }
    }

    /// An event back from its line; `None` for a line that is not one.
    pub fn from_line(line: &str) -> Option<Self> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let claim = |c: &&str| ClaimId((*c).to_owned());
        Some(match words.as_slice() {
            ["lease-start"] => Self::LeaseStart,
            ["wait"] => Self::Wait,
            ["handoff", c] => Self::Handoff(claim(c)),
            ["pull", c] => Self::Pull(claim(c)),
            ["refused", c] => Self::Refused(claim(c)),
            ["release", c] => Self::Release(claim(c)),
            ["submit", c, s] => Self::Submit {
                claim: claim(c),
                submission: SubmissionId((*s).to_owned()),
            },
            ["lease-end", why] => Self::LeaseEnd((*why).to_owned()),
            [
                "job-start",
                node,
                job,
                state @ ("waiting" | "running"),
                argv @ ..,
            ] => Self::JobStart {
                node: (*node).to_owned(),
                job: (*job).to_owned(),
                waiting: *state == "waiting",
                argv: argv.join(" "),
            },
            ["tool-call", exit, command @ ..] => Self::ToolCall {
                exit: exit.parse().ok(),
                command: command.join(" "),
            },
            ["job", node, job, status, verdict] => Self::JobEnd {
                node: (*node).to_owned(),
                job: (*job).to_owned(),
                status: (*status).to_owned(),
                verdict: (*verdict).to_owned(),
            },
            _ => return None,
        })
    }
}

/// A shell call a worker can cite as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvidence {
    pub id: String,
    pub command: String,
    pub exit: Option<i32>,
}

/// A recorded trace event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub lease: String,
    /// Order within the lease, from 1.
    pub seq: u64,
    /// When, as an RFC 3339 timestamp.
    pub at: String,
    pub event: TraceEvent,
}

/// What produced a head version: the submission, its claim and item, and the
/// lease that held the claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub submission: SubmissionId,
    pub claim: ClaimId,
    pub item: Item,
    pub attempt: u32,
    pub lease: String,
}

/// Where a lease runs: a pane on a node, started by the console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRecord {
    pub lease: String,
    pub node: String,
    /// The job on that node that is the pane.
    pub job: String,
    /// When it was recorded, in Unix seconds.
    pub at: Now,
    /// Who started it: `console` or `waker`.
    pub by: String,
}
