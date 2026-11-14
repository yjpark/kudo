//! The context store: a local kinora store holding contexts, items, claims,
//! settlements and submissions, each a kino. Nothing is edited in place: a
//! head moves by a new version, an item moves by a claim and a settlement.
//!
//! The shapes, all distinguished by `kudo::kind` metadata:
//!
//! - **context** — born as a card (a random marker, title, creation time —
//!   the marker keeps two same-title, same-second contexts apart); every head is a
//!   version of that identity, so the supersedes chain is the head history.
//! - **item** — write-once; the ask as typed, under a one-line marker that
//!   makes two identical asks two distinct kinos.
//! - **claim** — write-once, its content `(item, attempt)` only, so the same
//!   attempt can be claimed once; lease and deadline ride in metadata.
//! - **settle** — write-once, one per claim: `submitted`, `released`, or `blocked`.
//! - **submission** — write-once; the typed submission rendered in fixed
//!   sections (established lines whose evidence did not resolve land as
//!   claimed), linked to its claim, item, head and cited kinos.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e
//! implements: 6f2cfc2264e8f880d1300d1d79b96c597b995b36b756a03d6871a945b986f2dd@6f2cfc2264e8f880d1300d1d79b96c597b995b36b756a03d6871a945b986f2dd
//! implements: 3a472b4021f7b7496dd37779f3d0c76cafe1c95e8e80b1b236786a81e4c7d0ab@aaddfc2e7ee067ea5780ce6ee2dfe4587bfb47b0c067b2670a31308c019a7e21
//! implements: 4e67251bc978f34e7d6ce4278eb1503f9e803e51f0057d4d5954c701830f22b9@1b7ca6a98f08e1312c89a21ba22a046c2f9677965e58dd2fde0cae2756307a74
//! implements: b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9@b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9
//! implements: a6751902e8e01e5d7d5983196da9170f33f40d0d74246b5746977ea1c5e45098@a6751902e8e01e5d7d5983196da9170f33f40d0d74246b5746977ea1c5e45098

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use kinora::event::Event;
use kinora::hash::Hash;
use kinora::kino::{StoreKinoParams, store_kino};
use kinora::paths::kinora_root;
use kinora::resolve::Resolver;
use kinora::store::ContentStore;

use crate::{
    Claim, ClaimId, ContextId, ContextSummary, Error, Head, InboxEntry, Item, ItemId, Now,
    PaneRecord, Provenance, Repo, Submission, SubmissionId, Submitted, TraceEntry, TraceEvent,
};

const KIND: &str = "kudo::kind";
const CONTEXT: &str = "kudo::context";
const ITEM: &str = "kudo::item";
const CLAIM: &str = "kudo::claim";
const SEQ: &str = "kudo::seq";
const ATTEMPT: &str = "kudo::attempt";
const LEASE: &str = "kudo::lease";
const DEADLINE: &str = "kudo::deadline";
const OUTCOME: &str = "kudo::outcome";
const HEAD: &str = "kudo::head";
const BASE: &str = "kudo::base";
const EVIDENCE: &str = "kudo::evidence";
const ORIGIN: &str = "kudo::origin";
const EVENT: &str = "kudo::event";
const NODE: &str = "kudo::node";
const JOB: &str = "kudo::job";
const BY: &str = "kudo::by";
const SUBMISSION: &str = "kudo::submission";
const REASON: &str = "kudo::reason";
const ORIGIN_ID: &str = "kudo::origin_id";
const DUE: &str = "kudo::due";
const AFTER: &str = "kudo::after";
const BLOCKED_ON: &str = "kudo::blocked_on";
const CHECK: &str = "kudo::check";
const CHECK_PASS: &str = "kudo::check_pass";
const CHECK_WINDOW: &str = "kudo::check_window";
const SNOOZED_FROM: &str = "kudo::snoozed_from";
const VERDICT: &str = "kudo::verdict";

const SUBMITTED: &str = "submitted";
const RELEASED: &str = "released";
const BLOCKED: &str = "blocked";

/// A context store rooted at a directory.
pub struct ContextStore {
    /// The `.kinora` directory inside the store's directory.
    kinora: PathBuf,
    /// The object store this one syncs with, if any.
    mirror: Option<crate::Mirror>,
    /// The least time between two pulls a read may trigger.
    spacing: std::time::Duration,
    /// After the mirror could not be reached, how long to work locally.
    backoff: std::time::Duration,
    /// When the mirror was last pulled, last failed, last written to.
    sync_state: std::sync::Mutex<SyncState>,
}

/// The mirror's recent history within this process.
#[derive(Default)]
struct SyncState {
    last_pull: Option<std::time::Instant>,
    failed_at: Option<std::time::Instant>,
    last_write: Option<std::time::Instant>,
}

/// A pull this recent, with no local write since, makes a forced one
/// redundant (two calls of one operation, milliseconds apart).
const FRESH: std::time::Duration = std::time::Duration::from_millis(200);

/// What a failed sync means for the store.
enum Failure {
    /// The mirror cannot be reached: work locally for a while.
    Offline,
    /// The mirror refused this machine: an error to surface.
    Refused(String),
    /// Something local (an unreadable file): report it, sync the rest.
    Local,
}

fn failure(e: &kinora_remote::RemoteError) -> Failure {
    use kinora_remote::RemoteError as R;
    match e {
        e if e.is_refused() => Failure::Refused(e.to_string()),
        R::Io(_) => Failure::Local,
        _ => Failure::Offline,
    }
}

impl ContextStore {
    /// Create a new store in `dir` (which must not hold one yet).
    pub fn init(dir: &Path) -> Result<Self, Error> {
        kinora::init::init(dir, "kudo-context").map_err(kin)?;
        Self::open(dir)
    }

    /// Open an existing store in `dir`.
    pub fn open(dir: &Path) -> Result<Self, Error> {
        let kinora = kinora_root(dir);
        if !kinora.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no context store at {}", dir.display()),
            )));
        }
        Ok(Self {
            kinora,
            mirror: None,
            spacing: std::time::Duration::from_secs(1),
            backoff: std::time::Duration::from_secs(30),
            sync_state: std::sync::Mutex::new(SyncState::default()),
        })
    }

    /// Mirror this store: every operation pulls the mirror before it reads,
    /// and every write is pushed as it lands.
    pub fn with_mirror(mut self, mirror: crate::Mirror) -> Self {
        self.mirror = Some(mirror);
        self
    }

    /// The least time between two pulls a plain read may trigger (a second
    /// by default). A claim, a submit, a release and a repo declaration
    /// always pull first, whatever the spacing.
    pub fn with_sync_spacing(mut self, spacing: std::time::Duration) -> Self {
        self.spacing = spacing;
        self
    }

    /// After the mirror could not be reached, how long this process works
    /// locally before trying it again (30 seconds by default).
    pub fn with_offline_backoff(mut self, backoff: std::time::Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Submit for `lease`: refused unless `lease` holds the claim.
    pub fn submit_as(
        &self,
        lease: &str,
        claim: &ClaimId,
        submission: &Submission,
        now: Now,
    ) -> Result<Submitted, Error> {
        self.submit_checked(Some(lease), claim, submission, now)
    }

    /// Release for `lease`: refused unless `lease` holds the claim.
    pub fn release_as(&self, lease: &str, claim: &ClaimId, now: Now) -> Result<(), Error> {
        self.release_checked(Some(lease), claim, now)
    }

    /// Push what this store has not sent, then pull what it lacks. A mirror
    /// that cannot be reached is reported on stderr and the store goes on
    /// locally; what it could not send goes on the next sync.
    // implements: fc473f56b1b8bdbae4a31e54e7281c4aeed3c08bd03804b256ca37e1937fe66f@61632ce8ff602a5a85acd48cc9208e1ca388d7a696ce46d8459e27b85426538e
    fn sync(&self, force: bool) -> Result<(), Error> {
        let Some(mirror) = &self.mirror else {
            return Ok(());
        };
        let mut state = self
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .failed_at
            .is_some_and(|at| at.elapsed() < self.backoff)
        {
            return Ok(()); // offline a moment ago: work locally
        }
        let wrote_since = match (state.last_write, state.last_pull) {
            (Some(w), Some(p)) => w > p,
            (Some(_), None) => true,
            _ => false,
        };
        if let Some(at) = state.last_pull {
            let spaced = !force && at.elapsed() < self.spacing;
            let redundant = force && !wrote_since && at.elapsed() < FRESH;
            if spaced || redundant {
                return Ok(());
            }
        }
        if let Err(e) = kinora_remote::blocking::push(mirror, &self.kinora) {
            match failure(&e) {
                Failure::Refused(why) => return Err(Error::Mirror(why)),
                Failure::Offline => {
                    state.failed_at = Some(std::time::Instant::now());
                    eprintln!(
                        "kudo: context store mirror unreachable, working locally (push: {e})"
                    );
                    return Ok(());
                }
                // A local file it could not send: the pull still runs.
                Failure::Local => eprintln!("kudo: context store mirror push incomplete: {e}"),
            }
        }
        match kinora_remote::blocking::pull(mirror, &self.kinora) {
            Ok(_) => {
                state.last_pull = Some(std::time::Instant::now());
                state.failed_at = None;
                Ok(())
            }
            Err(e) => match failure(&e) {
                Failure::Refused(why) => Err(Error::Mirror(why)),
                Failure::Offline | Failure::Local => {
                    state.failed_at = Some(std::time::Instant::now());
                    eprintln!(
                        "kudo: context store mirror unreachable, working locally (pull: {e})"
                    );
                    Ok(())
                }
            },
        }
    }

    /// Push what was just written; offline, it waits for the next sync.
    fn push_now(&self) -> Result<(), Error> {
        let Some(mirror) = &self.mirror else {
            return Ok(());
        };
        let mut state = self
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.last_write = Some(std::time::Instant::now());
        if state
            .failed_at
            .is_some_and(|at| at.elapsed() < self.backoff)
        {
            return Ok(());
        }
        if let Err(e) = kinora_remote::blocking::push(mirror, &self.kinora) {
            match failure(&e) {
                Failure::Refused(why) => return Err(Error::Mirror(why)),
                Failure::Offline => {
                    state.failed_at = Some(std::time::Instant::now());
                    eprintln!("kudo: context store mirror unreachable, kept locally (push: {e})");
                }
                Failure::Local => eprintln!("kudo: context store mirror push incomplete: {e}"),
            }
        }
        Ok(())
    }

    /// Birth a context whose first head is `head`.
    pub fn create_context(&self, title: &str, head: &str, now: Now) -> Result<ContextId, Error> {
        // The marker makes the card unique: a title and a second do not.
        let card = format!(
            "<!-- kudo context {} -->\n# {title}\n\nkudo context, created {}\n",
            nonce()?,
            ts(now)
        );
        let born = self.put(
            "markdown",
            card.into_bytes(),
            meta([(KIND, "context")]),
            None,
            now,
        )?;
        let ctx = ContextId(born.id.clone());
        self.put(
            "markdown",
            head_content(&format!("{} new", ctx.0), head),
            meta([(KIND, "head")]),
            Some((born.id, vec![born.hash])),
            now,
        )?;
        Ok(ctx)
    }

    /// Every context in the store, newest first.
    pub fn contexts(&self) -> Result<Vec<ContextSummary>, Error> {
        let view = self.view()?;
        let mut cards = view.cards();
        cards.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.id.cmp(&b.id)));
        cards
            .into_iter()
            .map(|card| {
                let content = self.read(&card.hash)?;
                let title = strip_marker(&content)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("# ")
                    .to_owned();
                Ok(ContextSummary {
                    id: ContextId(card.id.clone()),
                    title,
                    created: card.ts.clone(),
                })
            })
            .collect()
    }

    /// The context `id_or_prefix` names: its full id, or a prefix of it that
    /// no other context shares (ids are all one length, so a full id is
    /// always unique).
    pub fn resolve_context(&self, id_or_prefix: &str) -> Result<ContextId, Error> {
        if id_or_prefix.is_empty() {
            return Err(Error::UnknownContext(String::new()));
        }
        let view = self.view()?;
        let matches: Vec<&Event> = view
            .cards()
            .into_iter()
            .filter(|card| card.id.starts_with(id_or_prefix))
            .collect();
        match matches.as_slice() {
            [one] => Ok(ContextId(one.id.clone())),
            [] => Err(Error::UnknownContext(id_or_prefix.to_owned())),
            many => Err(Error::AmbiguousContext(id_or_prefix.to_owned(), many.len())),
        }
    }

    /// Declare a repo the context spans; a repo of the same name on the same
    /// node moves to the new path.
    pub fn declare_repo(&self, ctx: &ContextId, repo: &Repo, now: Now) -> Result<(), Error> {
        self.sync(true)?;
        validate(repo)?;
        let view = self.view()?;
        view.require_context(ctx)?;
        let birth = format!("<!-- kudo repos {} -->\n# Repos\n", ctx.0);
        let id = Hash::of_content(birth.as_bytes()).as_hex().to_owned();
        let tips: Vec<String> = match view.by_id.contains_key(&id) {
            true => view.tips_of(&id).iter().map(|e| e.hash.clone()).collect(),
            false => vec![
                self.put(
                    "markdown",
                    birth.into_bytes(),
                    meta([(KIND, "repos"), (CONTEXT, &ctx.0)]),
                    None,
                    now,
                )?
                .hash,
            ],
        };
        let mut repos = self.repos_in(&view, ctx)?;
        repos.retain(|r| !(r.name == repo.name && r.node == repo.node));
        repos.push(repo.clone());
        repos.sort();
        // A marker of its own per version: a declaration that returns to an
        // earlier one's text is a new version, never a loop.
        let mut content = format!("<!-- kudo repos {} {} -->\n# Repos\n\n", ctx.0, nonce()?);
        for r in &repos {
            content.push_str(&format!(
                "- {} @ {}: {}\n",
                r.name,
                r.node,
                r.path.display()
            ));
        }
        self.put(
            "markdown",
            content.into_bytes(),
            meta([(KIND, "repos"), (CONTEXT, &ctx.0)]),
            Some((id, tips)),
            now,
        )?;
        Ok(())
    }

    /// The repos the context declares, by name then node.
    pub fn repos(&self, ctx: &ContextId) -> Result<Vec<Repo>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        self.repos_in(&view, ctx)
    }

    /// The repo of this name on this node.
    pub fn repo_on(&self, ctx: &ContextId, name: &str, node: &str) -> Result<Repo, Error> {
        self.repos(ctx)?
            .into_iter()
            .find(|r| r.name == name && r.node == node)
            .ok_or_else(|| Error::UnknownRepo {
                name: name.to_owned(),
                node: node.to_owned(),
            })
    }

    fn repos_in(&self, view: &View, ctx: &ContextId) -> Result<Vec<Repo>, Error> {
        let birth = format!("<!-- kudo repos {} -->\n# Repos\n", ctx.0);
        let id = Hash::of_content(birth.as_bytes()).as_hex().to_owned();
        // Every tip, newest first: a raced declaration forks the kino, and no
        // entry of either tip may be lost (on a clash the newer path wins).
        let mut repos: Vec<Repo> = Vec::new();
        for tip in view.tips_of(&id) {
            for line in self.read(&tip.hash)?.lines() {
                let Some((name, rest)) = line.strip_prefix("- ").and_then(|l| l.split_once(" @ "))
                else {
                    continue;
                };
                let Some((node, path)) = rest.split_once(": ") else {
                    continue;
                };
                if !repos.iter().any(|r| r.name == name && r.node == node) {
                    repos.push(Repo {
                        name: name.to_owned(),
                        node: node.to_owned(),
                        path: PathBuf::from(path),
                    });
                }
            }
        }
        repos.sort();
        Ok(repos)
    }

    /// Copy into this store every kino a submission cites from the ledger of
    /// a repo the context declares (cite-and-cache): the context then reads
    /// whole without the repo. A repo whose ledger is not on this machine is
    /// skipped.
    fn cache_cited(
        &self,
        view: &View,
        ctx: &ContextId,
        submission: &Submission,
        now: Now,
    ) -> Result<(), Error> {
        // Only ids the store holds no kino or version for go to the repo
        // ledgers (or ones another context cached): a bookkeeping id cited
        // by mistake costs no ledger load.
        let known = view.known(ctx);
        let held: std::collections::HashSet<&str> = view
            .by_id
            .iter()
            .flat_map(|(id, events)| {
                std::iter::once(id.as_str()).chain(events.iter().map(|e| e.hash.as_str()))
            })
            .collect();
        let wanted: Vec<String> = submission
            .established
            .iter()
            .flat_map(|l| l.evidence.iter().map(|e| normalize(e)))
            .filter(|e| {
                is_kino_id(e)
                    && !known.contains(e.as_str())
                    && (!held.contains(e.as_str()) || view.cited_elsewhere(e))
            })
            .collect();
        if wanted.is_empty() {
            return Ok(());
        }
        let mut cached = std::collections::HashSet::new();
        for repo in self.repos_in(view, ctx)? {
            let ledger = kinora_root(&repo.path);
            if !ledger.is_dir() {
                continue;
            }
            let Ok(resolver) = Resolver::load(&ledger) else {
                continue;
            };
            for want in &wanted {
                if cached.contains(want) {
                    continue;
                }
                let found = match resolver.identities().get(want.as_str()) {
                    Some(identity) => identity
                        .heads
                        .iter()
                        .max_by(|a, b| a.ts.cmp(&b.ts))
                        .cloned(),
                    None => resolver
                        .identities()
                        .values()
                        .flat_map(|i| i.events.iter())
                        .find(|e| e.hash == **want)
                        .cloned(),
                };
                let Some(event) = found else { continue };
                let Ok(hash) = event.hash.parse::<Hash>() else {
                    continue;
                };
                let Ok(bytes) = ContentStore::new(&ledger).read(&hash) else {
                    continue;
                };
                self.put(
                    &event.kind,
                    bytes,
                    meta([
                        (KIND, "cite"),
                        (CONTEXT, &ctx.0),
                        (ORIGIN, &repo.name),
                        (ORIGIN_ID, want),
                    ]),
                    None,
                    now,
                )?;
                cached.insert(want.clone());
            }
        }
        Ok(())
    }

    /// Record one loop event for `lease` under the context.
    pub fn record_trace(
        &self,
        ctx: &ContextId,
        lease: &str,
        event: &TraceEvent,
        now: Now,
    ) -> Result<String, Error> {
        self.record_trace_with(ctx, lease, event, "", now)
    }

    /// [`record_trace`], with a body below the event's line (a tool call's
    /// output); the trace kino's id.
    fn record_trace_with(
        &self,
        ctx: &ContextId,
        lease: &str,
        event: &TraceEvent,
        body: &str,
        now: Now,
    ) -> Result<String, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let seq = (view
            .of_kind_in("trace", ctx)
            .iter()
            .filter(|e| field(e, LEASE) == lease)
            .count()
            + 1)
        .to_string();
        // A job event's line is its whole record (node, job, state, argv or
        // verdict), kept as the reason and read back from it.
        let job_line = event.to_line();
        let (name, claim, submission, reason) = match event {
            TraceEvent::LeaseStart => ("lease-start", "", "", ""),
            TraceEvent::Handoff(c) => ("handoff", c.0.as_str(), "", ""),
            TraceEvent::Pull(c) => ("pull", c.0.as_str(), "", ""),
            TraceEvent::Refused(c) => ("refused", c.0.as_str(), "", ""),
            TraceEvent::Release(c) => ("release", c.0.as_str(), "", ""),
            TraceEvent::Submit { claim, submission } => {
                ("submit", claim.0.as_str(), submission.0.as_str(), "")
            }
            TraceEvent::LeaseEnd(why) => ("lease-end", "", "", why.as_str()),
            TraceEvent::Wait => ("wait", "", "", ""),
            TraceEvent::JobStart { .. }
            | TraceEvent::JobEnd { .. }
            | TraceEvent::ToolCall { .. } => (event.name(), "", "", job_line.as_str()),
        };
        let line = event.to_line();
        // The nonce keeps two equal events of one lease two kinos.
        let mut content = format!("<!-- kudo trace {lease} #{seq} {} -->\n{line}\n", nonce()?);
        if !body.is_empty() {
            content.push('\n');
            content.push_str(body);
            content.push('\n');
        }
        let born = self.put(
            "text",
            content.into_bytes(),
            meta([
                (KIND, "trace"),
                (CONTEXT, &ctx.0),
                (LEASE, lease),
                (SEQ, &seq),
                (EVENT, name),
                (CLAIM, claim),
                (SUBMISSION, submission),
                (REASON, reason),
            ]),
            None,
            now,
        )?;
        Ok(born.id)
    }

    /// Record a shell call `lease` made, as it returned: the command, its
    /// exit, and the first and last 2 KB of its output. The kino's id, which
    /// a submission of this context may cite.
    // implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
    pub fn record_tool_call(
        &self,
        ctx: &ContextId,
        lease: &str,
        command: &str,
        exit: Option<i32>,
        output: &str,
        now: Now,
    ) -> Result<String, Error> {
        let command = one_line(command);
        // The line (metadata) carries at most 512 bytes of the command; the
        // body keeps it, excerpted like the output.
        let mut cut = command.len().min(512);
        while !command.is_char_boundary(cut) {
            cut -= 1;
        }
        let event = TraceEvent::ToolCall {
            command: command[..cut].to_owned(),
            exit,
        };
        let exit = exit.map_or_else(|| "unknown".to_owned(), |c| c.to_string());
        let body = format!(
            "$ {}\n(exit {exit})\n\n{}",
            excerpt(&command, OUTPUT_KEPT),
            excerpt(output, OUTPUT_KEPT)
        );
        self.record_trace_with(ctx, lease, &event, &body, now)
    }

    /// Record that a job a worker started ended (`end`, a
    /// [`TraceEvent::JobEnd`]), with the first and last 2 KB of what it
    /// printed. The kino's id, which a submission of this context may cite.
    // implements: e23a8b06aa9331b3f1ffd8df36be1392daf44e93ca20037b50cec6c3132f53e1@d9656cc142296b3c9017b9d73ca59d326c9a29abd2945a5afd4a73eb6514f593
    pub fn record_job_end(
        &self,
        ctx: &ContextId,
        lease: &str,
        end: &TraceEvent,
        output: &str,
        now: Now,
    ) -> Result<String, Error> {
        let body = format!("output:\n\n{}", excerpt(output, OUTPUT_KEPT));
        self.record_trace_with(ctx, lease, end, &body, now)
    }

    /// A kino's text as stored (its first version), marker stripped.
    pub fn kino_text(&self, id: &str) -> Result<String, Error> {
        let view = self.view()?;
        let event = view
            .by_id
            .get(id)
            .and_then(|events| events.first())
            .ok_or_else(|| Error::Kinora(format!("no kino {id}")))?;
        Ok(strip_marker(&self.read(&event.hash)?))
    }

    /// The shell evidence recorded by one lease, oldest first.
    pub fn tool_evidence(
        &self,
        ctx: &ContextId,
        lease: &str,
    ) -> Result<Vec<crate::ToolEvidence>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut events: Vec<&Event> = view
            .of_kind_in("trace", ctx)
            .into_iter()
            .filter(|event| field(event, LEASE) == lease && field(event, EVENT) == "tool-call")
            .collect();
        events.sort_by_key(|event| seq_of(event));
        Ok(events
            .into_iter()
            .filter_map(|event| {
                let TraceEvent::ToolCall { command, exit } =
                    TraceEvent::from_line(field(event, REASON))?
                else {
                    return None;
                };
                Some(crate::ToolEvidence {
                    id: event.id.clone(),
                    command,
                    exit,
                })
            })
            .collect())
    }

    /// Record that `lease` runs in the pane `job` on `node`: written by the
    /// console when it starts a worker there, so the worker can be shown and
    /// taken over.
    pub fn record_pane(
        &self,
        ctx: &ContextId,
        lease: &str,
        node: &str,
        job: &str,
        now: Now,
    ) -> Result<(), Error> {
        self.record_pane_by(ctx, lease, node, job, "console", now)
    }

    /// Record a pane, naming who started it (`console` or `waker`): the
    /// waker stops only the panes it started.
    // implements: 43643bef4da543e855bafcb2308401df3c7b2f1c5a4ef03ac69c06b7a4bd2d2a
    pub fn record_pane_by(
        &self,
        ctx: &ContextId,
        lease: &str,
        node: &str,
        job: &str,
        by: &str,
        now: Now,
    ) -> Result<(), Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let content = format!(
            "<!-- kudo pane {lease} {} -->\npane {lease} {node} {job}\n",
            nonce()?
        );
        self.put(
            "text",
            content.into_bytes(),
            meta([
                (KIND, "pane"),
                (CONTEXT, &ctx.0),
                (LEASE, lease),
                (NODE, node),
                (JOB, job),
                (BY, by),
            ]),
            None,
            now,
        )?;
        Ok(())
    }

    /// The panes recorded under the context.
    pub fn panes(&self, ctx: &ContextId) -> Result<Vec<PaneRecord>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut events = view.of_kind_in("pane", ctx);
        events.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
        Ok(events
            .into_iter()
            .map(|e| PaneRecord {
                lease: field(e, LEASE).to_owned(),
                node: field(e, NODE).to_owned(),
                job: field(e, JOB).to_owned(),
                at: e.ts.parse::<jiff::Timestamp>().map_or(0, |t| t.as_second()),
                // Panes recorded before this field were the console's.
                by: Some(field(e, BY))
                    .filter(|b| !b.is_empty())
                    .unwrap_or("console")
                    .to_owned(),
            })
            .collect())
    }

    /// The context's trace: every lease's events, in order.
    pub fn trace(&self, ctx: &ContextId) -> Result<Vec<TraceEntry>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut events = view.of_kind_in("trace", ctx);
        events.sort_by(|a, b| {
            (a.ts.as_str(), field(a, LEASE), seq_of(a)).cmp(&(
                b.ts.as_str(),
                field(b, LEASE),
                seq_of(b),
            ))
        });
        // Leases interleave by the clock, but a lease's own events keep its
        // own order (seq): a clock that stepped back must not reorder them.
        let mut by_lease: HashMap<&str, Vec<&Event>> = HashMap::new();
        for e in &events {
            by_lease.entry(field(e, LEASE)).or_default().push(e);
        }
        for own in by_lease.values_mut() {
            own.sort_by_key(|e| std::cmp::Reverse(seq_of(e)));
        }
        let events: Vec<&Event> = events
            .iter()
            .filter_map(|e| by_lease.get_mut(field(e, LEASE)).and_then(Vec::pop))
            .collect();
        Ok(events
            .into_iter()
            .filter_map(|e| {
                let claim = || ClaimId(field(e, CLAIM).to_owned());
                let event = match field(e, EVENT) {
                    "lease-start" => TraceEvent::LeaseStart,
                    "wait" => TraceEvent::Wait,
                    "handoff" => TraceEvent::Handoff(claim()),
                    "pull" => TraceEvent::Pull(claim()),
                    "refused" => TraceEvent::Refused(claim()),
                    "release" => TraceEvent::Release(claim()),
                    "submit" => TraceEvent::Submit {
                        claim: claim(),
                        submission: SubmissionId(field(e, SUBMISSION).to_owned()),
                    },
                    "lease-end" => TraceEvent::LeaseEnd(field(e, REASON).to_owned()),
                    "job-start" | "job" | "tool-call" => TraceEvent::from_line(field(e, REASON))?,
                    _ => return None,
                };
                Some(TraceEntry {
                    lease: field(e, LEASE).to_owned(),
                    seq: seq_of(e),
                    at: e.ts.clone(),
                    event,
                })
            })
            .collect())
    }

    /// What produced the head version `version`; `None` for the first head.
    pub fn provenance(&self, ctx: &ContextId, version: &str) -> Result<Option<Provenance>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        // A head version names the claim that wrote it; the first head none.
        let Some(head) = view
            .by_id
            .get(&ctx.0)
            .and_then(|events| events.iter().find(|e| e.hash == version))
        else {
            return Err(Error::Kinora(format!(
                "no head version {version} in {}",
                ctx.0
            )));
        };
        let claim = field(head, CLAIM);
        if claim.is_empty() {
            return Ok(None);
        }
        let claim_event = view
            .by_id
            .get(claim)
            .and_then(|events| claim_birth(events))
            .ok_or_else(|| Error::UnknownClaim(claim.to_owned()))?;
        let submission = view
            .of_kind_in("submission", ctx)
            .into_iter()
            .find(|e| field(e, CLAIM) == claim)
            .ok_or_else(|| {
                Error::Kinora(format!("claim {claim} wrote a head but no submission"))
            })?;
        let item = view
            .by_id
            .get(field(claim_event, ITEM))
            .and_then(|events| birth(events))
            .ok_or_else(|| Error::Kinora(format!("claim {claim} names a missing item")))?;
        Ok(Some(Provenance {
            submission: SubmissionId(submission.id.clone()),
            claim: ClaimId(claim.to_owned()),
            item: self.item_of(item)?,
            attempt: field(claim_event, ATTEMPT).parse().unwrap_or(1),
            lease: field(claim_event, LEASE).to_owned(),
        }))
    }

    /// The context's current head.
    pub fn head(&self, ctx: &ContextId) -> Result<Head, Error> {
        let view = self.view()?;
        let chain = view.head_chain(ctx)?;
        let last = chain
            .last()
            .ok_or_else(|| Error::UnknownContext(ctx.0.clone()))?;
        self.head_of(last)
    }

    /// The head's current versions, newest first: one, unless two submits
    /// raced and forked it.
    pub fn heads(&self, ctx: &ContextId) -> Result<Vec<Head>, Error> {
        let view = self.view()?;
        view.tips(ctx)?.iter().map(|e| self.head_of(e)).collect()
    }

    /// One version of a head, by its hash.
    pub fn head_version(&self, version: &str) -> Result<Head, Error> {
        Ok(Head {
            version: version.to_owned(),
            body: strip_marker(&self.read(version)?),
        })
    }

    /// Every version of the head, oldest first.
    pub fn head_history(&self, ctx: &ContextId) -> Result<Vec<Head>, Error> {
        let view = self.view()?;
        view.head_chain(ctx)?
            .iter()
            .map(|e| self.head_of(e))
            .collect()
    }

    /// Enqueue `ask` under the context.
    pub fn enqueue(&self, ctx: &ContextId, ask: &str, now: Now) -> Result<ItemId, Error> {
        self.enqueue_item(ctx, ask, None, None, None, &[], now)
    }

    /// Enqueue `ask`, gated by dependencies and optionally a due time.
    // implements: a99975ca76ea8d354544294d35af3d49c1215a2029be4aa91f51e541fc26fdc7
    pub fn enqueue_with_dependencies(
        &self,
        ctx: &ContextId,
        ask: &str,
        due: Option<Now>,
        after: &[ItemId],
        now: Now,
    ) -> Result<ItemId, Error> {
        self.enqueue_item(ctx, ask, due, None, None, after, now)
    }

    /// Enqueue `ask`, ready at `due` (epoch seconds) or at once.
    // implements: 716432a845eb22babf957938aaa9a9c0f3cd3bea3ebee24c89a5147a795e7e2b@80a3c962223cc4b37ffceccfdfb83cb9d143fd3e956d4cbe19c7ec3e93774ffd
    pub fn enqueue_due(
        &self,
        ctx: &ContextId,
        ask: &str,
        due: Option<Now>,
        now: Now,
    ) -> Result<ItemId, Error> {
        self.enqueue_item(ctx, ask, due, None, None, &[], now)
    }

    /// Enqueue a check: what to look at, its pass condition and evidence
    /// window, ready at `due` or at once.
    // implements: c11043084f97c61ff3f41e4d07fb2361aa871213021e9c15d0e4cee37c99bde1
    pub fn enqueue_check(
        &self,
        ctx: &ContextId,
        check: &crate::Check,
        due: Option<Now>,
        now: Now,
    ) -> Result<ItemId, Error> {
        if check.what.trim().is_empty() || check.pass.trim().is_empty() {
            return Err(Error::InvalidCheck(
                "a check needs what to look at and a pass condition".into(),
            ));
        }
        self.enqueue_item(ctx, &check.what, due, Some(check), None, &[], now)
    }

    fn enqueue_item(
        &self,
        ctx: &ContextId,
        ask: &str,
        due: Option<Now>,
        check: Option<&crate::Check>,
        snoozed_from: Option<&ItemId>,
        after: &[ItemId],
        now: Now,
    ) -> Result<ItemId, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        view.validate_dependencies(ctx, after, None)?;
        let seq = (view.of_kind_in("item", ctx).len() + 1).to_string();
        // A snooze's successor names its origin in its bytes, so two machines
        // that snooze the same check write the same id.
        let content = match snoozed_from {
            Some(origin) => format!("<!-- kudo item {} snoozed {} -->\n{ask}", ctx.0, origin.0),
            None => format!("<!-- kudo item {} #{seq} -->\n{ask}", ctx.0),
        };
        let mut metadata = meta([(KIND, "item"), (CONTEXT, &ctx.0), (SEQ, &seq)]);
        if let Some(due) = due {
            metadata.insert(DUE.to_owned(), due.to_string());
        }
        if !after.is_empty() {
            metadata.insert(
                AFTER.to_owned(),
                after
                    .iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if let Some(check) = check {
            metadata.insert(CHECK.to_owned(), "true".to_owned());
            metadata.insert(CHECK_PASS.to_owned(), one_line(&check.pass));
            metadata.insert(CHECK_WINDOW.to_owned(), one_line(&check.window));
        }
        if let Some(origin) = snoozed_from {
            metadata.insert(SNOOZED_FROM.to_owned(), origin.0.clone());
        }
        let born = self.put("markdown", content.into_bytes(), metadata, None, now)?;
        Ok(ItemId(born.id))
    }

    /// How many items with a due time are ready now: what the waker wakes
    /// for (an item with no due time waits for a person).
    // implements: 43643bef4da543e855bafcb2308401df3c7b2f1c5a4ef03ac69c06b7a4bd2d2a@f76f4215e264261e284792a84ffb31b5fbc7f3498b04b00dccf45aa79cac60a2
    pub fn due_ready_count(&self, ctx: &ContextId, now: Now) -> Result<usize, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        Ok(view
            .ready(ctx, now)
            .iter()
            .filter(|(item, _)| due_of(item).is_some())
            .count())
    }

    /// The items not yet due, soonest first.
    pub fn not_yet_due(&self, ctx: &ContextId, now: Now) -> Result<Vec<Item>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut later: Vec<&Event> = view
            .of_kind_in("item", ctx)
            .into_iter()
            .filter(|item| due_of(item).is_some_and(|due| due > now))
            .filter(|item| view.first_successor(item))
            .filter(|item| {
                !view
                    .claims_on(&item.id)
                    .iter()
                    .any(|(_, outcome)| *outcome == Some(SUBMITTED))
            })
            .collect();
        later.sort_by_key(|item| (due_of(item), seq_of(item)));
        later.into_iter().map(|item| self.item_of(item)).collect()
    }

    /// Declare the node this context's woken workers start on; the latest
    /// declaration wins.
    // implements: 4afb77c56fe120a92c8ecb4f01b9a8873e9a41fb8eecb450b9a60e1b1ec327c0@550531c07103cb27e03f2d4362bcb4ed367debf0a67293484865999f716c77a4
    pub fn declare_wake_node(&self, ctx: &ContextId, node: &str, now: Now) -> Result<(), Error> {
        let node = node.trim();
        if node.is_empty() || node.contains(char::is_whitespace) {
            return Err(Error::InvalidNode(node.to_owned()));
        }
        let view = self.view()?;
        view.require_context(ctx)?;
        let seq = (view.of_kind_in("wake", ctx).len() + 1).to_string();
        let content = format!("<!-- kudo wake {} {} -->\nwake {node}\n", ctx.0, nonce()?);
        self.put(
            "text",
            content.into_bytes(),
            meta([(KIND, "wake"), (CONTEXT, &ctx.0), (NODE, node), (SEQ, &seq)]),
            None,
            now,
        )?;
        Ok(())
    }

    /// Where this context's woken workers start: the declared wake node,
    /// else its first repo's node (by name), else `None` (the waker's own).
    // implements: 4afb77c56fe120a92c8ecb4f01b9a8873e9a41fb8eecb450b9a60e1b1ec327c0@550531c07103cb27e03f2d4362bcb4ed367debf0a67293484865999f716c77a4
    pub fn wake_node(&self, ctx: &ContextId) -> Result<Option<String>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let declared = view
            .of_kind_in("wake", ctx)
            .into_iter()
            .max_by_key(|e| (e.ts.clone(), seq_of(e), e.hash.clone()))
            .map(|e| field(e, NODE).to_owned());
        if declared.is_some() {
            return Ok(declared);
        }
        Ok(self
            .repos_in(&view, ctx)?
            .into_iter()
            .next()
            .map(|r| r.node))
    }

    /// Whether an item is ready to pull: not submitted, and not held by a live
    /// claim.
    pub fn has_ready(&self, ctx: &ContextId, now: Now) -> Result<bool, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        Ok(view.next_ready(ctx, now).is_some())
    }

    /// How many items are ready to pull.
    pub fn ready_count(&self, ctx: &ContextId, now: Now) -> Result<usize, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        Ok(view.ready(ctx, now).len())
    }

    /// Items whose dependencies are not all submitted yet.
    pub fn waiting_count(&self, ctx: &ContextId, now: Now) -> Result<usize, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        Ok(view
            .of_kind_in("item", ctx)
            .into_iter()
            .filter(|item| due_of(item).is_none_or(|due| due <= now))
            .filter(|item| view.first_successor(item))
            .filter(|item| !view.dependencies_met(item))
            .filter(|item| {
                !view
                    .claims_on(&item.id)
                    .iter()
                    .any(|(_, outcome)| *outcome == Some(SUBMITTED))
            })
            .count())
    }

    /// Items finished by a normal submit, excluding blocked attempts.
    pub fn submitted_count(&self, ctx: &ContextId) -> Result<usize, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        Ok(view
            .of_kind_in("item", ctx)
            .into_iter()
            .filter(|item| {
                view.claims_on(&item.id)
                    .iter()
                    .any(|(_, outcome)| *outcome == Some(SUBMITTED))
            })
            .count())
    }

    /// Claim the oldest ready item for `lease`, with a deadline `ttl` seconds
    /// from `now`. `None` when nothing is ready.
    pub fn pull(
        &self,
        ctx: &ContextId,
        lease: &str,
        now: Now,
        ttl: i64,
    ) -> Result<Option<Claim>, Error> {
        self.sync(true)?;
        let view = self.view()?;
        view.require_context(ctx)?;
        let Some((item_event, attempt)) = view.next_ready(ctx, now) else {
            return Ok(None);
        };
        let item = self.item_of(item_event)?;
        let deadline = now + ttl;

        // The content is (item, attempt) and nothing else, so an attempt can be
        // claimed once: a second writer computes the same id and finds it taken.
        let content = format!("kudo claim\nitem: {}\nattempt: {attempt}\n", item.id.0);
        if view
            .by_id
            .contains_key(Hash::of_content(content.as_bytes()).as_hex())
        {
            return Ok(None);
        }
        let base: Vec<String> = view.tips(ctx)?.iter().map(|e| e.hash.clone()).collect();
        let base_s = base.join(",");
        let attempt_s = attempt.to_string();
        let deadline_s = deadline.to_string();
        let born = self.put(
            "text",
            content.into_bytes(),
            meta([
                (KIND, "claim"),
                (CONTEXT, &ctx.0),
                (ITEM, &item.id.0),
                (ATTEMPT, &attempt_s),
                (LEASE, lease),
                (DEADLINE, &deadline_s),
                (BASE, &base_s),
            ]),
            None,
            now,
        )?;
        let id = ClaimId(born.id);
        if !self.confirm(&id, lease)? {
            return Ok(None); // lost a race for this attempt
        }
        Ok(Some(Claim {
            id,
            context: ctx.clone(),
            item,
            attempt,
            lease: lease.to_owned(),
            deadline,
            base,
        }))
    }

    /// Submit the work for a live claim: the head is superseded by
    /// `submission.head`, the log entry is stored, and the claim is settled.
    pub fn submit(
        &self,
        claim: &ClaimId,
        submission: &Submission,
        now: Now,
    ) -> Result<Submitted, Error> {
        self.submit_checked(None, claim, submission, now)
    }

    // implements: 8c1fc5e54c5e48bb4086bd6b27a2d09bf41e100de7c743c6b04fd44b48af3d54
    fn submit_checked(
        &self,
        lease: Option<&str>,
        claim: &ClaimId,
        submission: &Submission,
        now: Now,
    ) -> Result<Submitted, Error> {
        self.sync(true)?;
        let view = self.view()?;
        let claim_event = view.live_claim(claim, now)?;
        held_by_lease(claim_event, claim, lease)?;
        let ctx = ContextId(field(claim_event, CONTEXT).to_owned());
        let item = field(claim_event, ITEM).to_owned();

        // A check closes with a verdict; nothing else takes one.
        // implements: c11043084f97c61ff3f41e4d07fb2361aa871213021e9c15d0e4cee37c99bde1
        let item_event = view
            .by_id
            .get(&item)
            .and_then(|events| events.first())
            .ok_or_else(|| Error::UnknownClaim(claim.0.clone()))?;
        let is_check = field(item_event, CHECK) == "true";
        match (&submission.check, is_check) {
            (None, true) => return Err(Error::CheckNeedsVerdict(claim.0.clone())),
            (Some(_), false) => return Err(Error::NotACheck(claim.0.clone())),
            (Some(close), true) => {
                if close.note.trim().is_empty() {
                    return Err(Error::InvalidCheck(
                        "a verdict needs a note: say what was seen".into(),
                    ));
                }
                if let crate::CheckVerdict::Snooze { until } = close.verdict
                    && until <= now
                {
                    return Err(Error::InvalidCheck(format!(
                        "a snooze looks again later: {until} is not after now ({now})"
                    )));
                }
            }
            (None, false) => {}
        }
        let snoozed = self.item_of(item_event)?;
        let blocked = !submission.blocked_on.is_empty();
        if blocked && is_check {
            return Err(Error::InvalidCheck(
                "a check closes with a verdict, not a blocked submission".into(),
            ));
        }
        view.validate_dependencies(&ctx, &submission.blocked_on, Some(&item))?;

        let tips = view.tips(&ctx)?;
        let current: Vec<String> = tips.iter().map(|e| e.hash.clone()).collect();
        let head_version = if blocked {
            current
                .first()
                .cloned()
                .ok_or_else(|| Error::Kinora("a live context has no head".into()))?
        } else {
            // The new head must build on every current tip, closing any fork.
            let mut base = submission
                .base
                .clone()
                .unwrap_or_else(|| base_of(claim_event));
            base.sort();
            let mut now_sorted = current.clone();
            now_sorted.sort();
            // A claim taken before claims recorded their base has none.
            if !base.is_empty() && base != now_sorted {
                return Err(Error::HeadMoved {
                    claim: claim.0.clone(),
                    current,
                });
            }
            let unchanged = match tips.as_slice() {
                [only] if self.head_of(only)?.body == submission.head => Some(only.hash.clone()),
                _ => None,
            };
            match unchanged {
                Some(version) => version,
                None => {
                    self.put(
                        "markdown",
                        head_content(&claim.0, &submission.head),
                        meta([(KIND, "head"), (CLAIM, &claim.0)]),
                        Some((ctx.0.clone(), current)),
                        now,
                    )?
                    .hash
                }
            }
        };

        self.cache_cited(&view, &ctx, submission, now)?;
        let view = self.view()?;
        let checked = view.check_evidence(&ctx, submission);
        let seq = (view.of_kind_in("submission", &ctx).len() + 1).to_string();
        let mut content = format!(
            "<!-- kudo submission {} -->\n{}",
            claim.0,
            render(submission, &checked, &head_version)
        );
        let verdict = submission.check.as_ref().map(|close| match close.verdict {
            crate::CheckVerdict::Pass => "pass".to_owned(),
            crate::CheckVerdict::Fail => "fail".to_owned(),
            crate::CheckVerdict::Snooze { until } => format!("snooze until {}", ts(until)),
        });
        if let (Some(close), Some(verdict)) = (&submission.check, &verdict) {
            content.push_str(&format!(
                "\n## Check\n\nverdict: {verdict}\nnote: {}\n",
                one_line(&close.note)
            ));
        }
        if blocked {
            content.push_str(&format!(
                "\n## Blocked\n\nwaiting on: {}\n",
                submission
                    .blocked_on
                    .iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let cited = checked.kinos().join(",");
        let mut metadata = meta([
            (KIND, "submission"),
            (CONTEXT, &ctx.0),
            (ITEM, &item),
            (CLAIM, &claim.0),
            (SEQ, &seq),
            (HEAD, &head_version),
            (EVIDENCE, &cited),
        ]);
        if let Some(verdict) = &verdict {
            metadata.insert(VERDICT.to_owned(), verdict.clone());
        }
        if blocked {
            metadata.insert(
                BLOCKED_ON.to_owned(),
                submission
                    .blocked_on
                    .iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        let born = self.put("markdown", content.into_bytes(), metadata, None, now)?;
        // A snooze looks again later: the same check, due then — queued
        // before the claim settles, so a failed write leaves the claim live
        // to submit again, never a closed check with nothing after it. A retry
        // finds the successor already there and queues no second one.
        let requeued = match submission.check.as_ref().map(|c| c.verdict) {
            Some(crate::CheckVerdict::Snooze { until }) => {
                let view = self.view()?;
                let existing = view
                    .of_kind_in("item", &ctx)
                    .into_iter()
                    .filter(|e| field(e, SNOOZED_FROM) == snoozed.id.0)
                    .min_by(|a, b| a.hash.cmp(&b.hash))
                    .map(|e| ItemId(e.id.clone()));
                match existing {
                    Some(id) => Some(id),
                    None => Some(self.enqueue_item(
                        &ctx,
                        &snoozed.ask,
                        Some(until),
                        snoozed.check.as_ref(),
                        Some(&snoozed.id),
                        &[],
                        now,
                    )?),
                }
            }
            _ => None,
        };
        self.settle(
            &ctx,
            claim,
            &item,
            if blocked { BLOCKED } else { SUBMITTED },
            &submission.blocked_on,
            now,
        )?;
        Ok(Submitted {
            id: SubmissionId(born.id),
            downgraded: checked.downgraded,
            unresolved: checked.unresolved,
            requeued,
        })
    }

    /// Give a live claim back: its item is ready again, as the next attempt.
    pub fn release(&self, claim: &ClaimId, now: Now) -> Result<(), Error> {
        self.release_checked(None, claim, now)
    }

    fn release_checked(&self, lease: Option<&str>, claim: &ClaimId, now: Now) -> Result<(), Error> {
        self.sync(true)?;
        let view = self.view()?;
        let claim_event = view.live_claim(claim, now)?;
        held_by_lease(claim_event, claim, lease)?;
        let ctx = ContextId(field(claim_event, CONTEXT).to_owned());
        let item = field(claim_event, ITEM).to_owned();
        self.settle(&ctx, claim, &item, RELEASED, &[], now)
    }

    /// The lease a claim was taken by.
    pub fn claim_lease(&self, claim: &ClaimId) -> Result<String, Error> {
        let view = self.view()?;
        view.by_id
            .get(&claim.0)
            .and_then(|events| claim_birth(events))
            .map(|e| field(e, LEASE).to_owned())
            .ok_or_else(|| Error::UnknownClaim(claim.0.clone()))
    }

    /// The live claim `lease` holds under the context, if any.
    pub fn held_by(&self, ctx: &ContextId, lease: &str, now: Now) -> Result<Option<Claim>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let held = view
            .by_id
            .values()
            .filter_map(|events| claim_birth(events))
            .filter(|e| field(e, CONTEXT) == ctx.0 && field(e, LEASE) == lease)
            .find(|e| view.live_claim(&ClaimId(e.id.clone()), now).is_ok());
        let Some(claim) = held else {
            return Ok(None);
        };
        let item = view
            .by_id
            .get(field(claim, ITEM))
            .and_then(|events| birth(events))
            .ok_or_else(|| Error::Kinora(format!("claim {} names a missing item", claim.id)))?;
        Ok(Some(Claim {
            id: ClaimId(claim.id.clone()),
            context: ctx.clone(),
            item: self.item_of(item)?,
            attempt: field(claim, ATTEMPT).parse().unwrap_or(1),
            lease: lease.to_owned(),
            deadline: deadline_of(claim),
            base: base_of(claim),
        }))
    }

    /// File a rough edge under the context: one line, an optional body, and
    /// the lease that filed it.
    pub fn file_inbox(
        &self,
        ctx: &ContextId,
        lease: &str,
        line: &str,
        body: Option<&str>,
        now: Now,
    ) -> Result<String, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let seq = (view.of_kind_in("inbox", ctx).len() + 1).to_string();
        let mut content = format!("<!-- kudo inbox {} #{seq} -->\n# {}\n", ctx.0, line.trim());
        if let Some(body) = body.filter(|b| !b.trim().is_empty()) {
            content.push('\n');
            content.push_str(body.trim_end());
            content.push('\n');
        }
        let born = self.put(
            "markdown",
            content.into_bytes(),
            meta([
                (KIND, "inbox"),
                (CONTEXT, &ctx.0),
                (LEASE, lease),
                (SEQ, &seq),
            ]),
            None,
            now,
        )?;
        Ok(born.id)
    }

    /// The context's inbox, oldest first.
    pub fn inbox(&self, ctx: &ContextId) -> Result<Vec<InboxEntry>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut entries = view.of_kind_in("inbox", ctx);
        entries.sort_by_key(|e| seq_of(e));
        entries
            .into_iter()
            .map(|e| {
                let text = strip_marker(&self.read(&e.hash)?);
                let (first, rest) = text.split_once('\n').unwrap_or((&text, ""));
                let body = rest.trim();
                Ok(InboxEntry {
                    line: first.trim_start_matches("# ").to_owned(),
                    body: (!body.is_empty()).then(|| body.to_owned()),
                    lease: field(e, LEASE).to_owned(),
                })
            })
            .collect()
    }

    /// The context's log: its submissions' entries, oldest first.
    pub fn log(&self, ctx: &ContextId) -> Result<Vec<String>, Error> {
        let view = self.view()?;
        view.require_context(ctx)?;
        let mut subs = view.of_kind_in("submission", ctx);
        subs.sort_by_key(|e| seq_of(e));
        subs.into_iter()
            .map(|e| Ok(strip_marker(&self.read(&e.hash)?)))
            .collect()
    }

    /// Whether `lease` holds the claim. Two pulls racing for the same attempt
    /// both write it — the same id, two births; the birth with the smallest
    /// lease wins, and the loser must back off.
    pub(crate) fn confirm(&self, claim: &ClaimId, lease: &str) -> Result<bool, Error> {
        // The other machine's birth of this claim may have reached the
        // mirror while this one wrote its own.
        self.sync(true)?;
        let view = self.view()?;
        let holder = view
            .by_id
            .get(&claim.0)
            .and_then(|events| claim_birth(events));
        Ok(holder.is_some_and(|e| field(e, LEASE) == lease))
    }

    fn settle(
        &self,
        ctx: &ContextId,
        claim: &ClaimId,
        item: &str,
        outcome: &str,
        blocked_on: &[ItemId],
        now: Now,
    ) -> Result<(), Error> {
        let content = format!("kudo settle\nclaim: {}\noutcome: {outcome}\n", claim.0);
        let mut metadata = meta([
            (KIND, "settle"),
            (CONTEXT, &ctx.0),
            (CLAIM, &claim.0),
            (ITEM, item),
            (OUTCOME, outcome),
        ]);
        if !blocked_on.is_empty() {
            metadata.insert(
                BLOCKED_ON.to_owned(),
                blocked_on
                    .iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        self.put("text", content.into_bytes(), metadata, None, now)?;
        Ok(())
    }

    /// Store one kino: a birth when `version_of` is `None`, else a version of
    /// that identity with the given parents.
    fn put(
        &self,
        kind: &str,
        content: Vec<u8>,
        metadata: BTreeMap<String, String>,
        version_of: Option<(String, Vec<String>)>,
        now: Now,
    ) -> Result<Event, Error> {
        let (id, parents) = match version_of {
            Some((id, parents)) => (Some(id), parents),
            None => (None, Vec::new()),
        };
        let stored = store_kino(
            &self.kinora,
            StoreKinoParams {
                kind: kind.to_owned(),
                content,
                author: "kudo".to_owned(),
                provenance: "kudo-context".to_owned(),
                ts: ts(now),
                metadata,
                id,
                parents,
            },
        )
        .map_err(kin)?;
        self.push_now()?;
        Ok(stored.event)
    }

    fn read(&self, hash: &str) -> Result<String, Error> {
        let hash: Hash = hash.parse().map_err(kin)?;
        let bytes = ContentStore::new(&self.kinora).read(&hash).map_err(kin)?;
        String::from_utf8(bytes).map_err(kin)
    }

    fn head_of(&self, event: &Event) -> Result<Head, Error> {
        self.head_version(&event.hash)
    }

    fn item_of(&self, event: &Event) -> Result<Item, Error> {
        Ok(Item {
            id: ItemId(event.id.clone()),
            seq: seq_of(event),
            ask: strip_marker(&self.read(&event.hash)?),
            due: due_of(event),
            after: split_ids(field(event, AFTER))
                .into_iter()
                .map(|id| ItemId(id.to_owned()))
                .collect(),
            check: (field(event, CHECK) == "true").then(|| crate::Check {
                what: String::new(),
                pass: field(event, CHECK_PASS).to_owned(),
                window: field(event, CHECK_WINDOW).to_owned(),
            }),
            snoozed_from: Some(field(event, SNOOZED_FROM))
                .filter(|s| !s.is_empty())
                .map(|s| ItemId(s.to_owned())),
        })
        .map(|mut item: Item| {
            if let Some(check) = &mut item.check {
                check.what = item.ask.clone();
            }
            item
        })
    }

    fn view(&self) -> Result<View, Error> {
        self.sync(false)?;
        View::load(&self.kinora)
    }
}

/// One read of the whole store, indexed for the queries above.
struct View {
    /// Every identity's events, by id.
    by_id: HashMap<String, Vec<Event>>,
}

impl View {
    fn load(kinora: &Path) -> Result<Self, Error> {
        let resolver = Resolver::load(kinora).map_err(kin)?;
        let by_id = resolver
            .identities()
            .iter()
            .map(|(id, identity)| (id.clone(), identity.events.clone()))
            .collect();
        Ok(Self { by_id })
    }

    /// The birth event of each write-once kino of `kind` under `ctx`.
    fn of_kind_in(&self, kind: &str, ctx: &ContextId) -> Vec<&Event> {
        self.by_id
            .values()
            .filter_map(|events| events.first())
            .filter(|e| field(e, KIND) == kind && field(e, CONTEXT) == ctx.0)
            .collect()
    }

    /// The card (birth event) of every context.
    fn cards(&self) -> Vec<&Event> {
        self.by_id
            .values()
            .filter_map(|events| birth(events))
            .filter(|e| field(e, KIND) == "context")
            .collect()
    }

    fn require_context(&self, ctx: &ContextId) -> Result<(), Error> {
        match self.by_id.get(&ctx.0).and_then(|events| birth(events)) {
            Some(card) if field(card, KIND) == "context" => Ok(()),
            _ => Err(Error::UnknownContext(ctx.0.clone())),
        }
    }

    /// The head versions of a context, oldest first (the card excluded).
    fn head_chain(&self, ctx: &ContextId) -> Result<Vec<&Event>, Error> {
        self.require_context(ctx)?;
        let events = self
            .by_id
            .get(&ctx.0)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let by_hash: HashMap<&str, &Event> = events.iter().map(|e| (e.hash.as_str(), e)).collect();
        // On a fork, follow the newest tip; a merge's first parent is the
        // newest tip it closed.
        let mut chain = Vec::new();
        let mut cursor = self.tips(ctx)?.first().copied();
        while let Some(e) = cursor {
            if e.parents.is_empty() {
                break; // the card
            }
            chain.push(e);
            cursor = e
                .parents
                .first()
                .and_then(|p| by_hash.get(p.as_str()).copied());
        }
        chain.reverse();
        Ok(chain)
    }

    /// Every id that can back a fact under `ctx`: kinos it cached from a
    /// declared repo's ledger (by their own id and their origin's) and its
    /// earlier submissions. The queue's own records — contexts, heads, items,
    /// claims, settles, repos, inbox lines — back nothing, and nothing of
    /// another context counts.
    fn known(&self, ctx: &ContextId) -> std::collections::HashSet<&str> {
        self.by_id
            .iter()
            .filter(|(_, events)| {
                events.iter().any(|b| {
                    b.parents.is_empty()
                        && field(b, CONTEXT) == ctx.0
                        && (matches!(field(b, KIND), "cite" | "submission")
                            // The loop's own records of what ran.
                            // implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
                            || (field(b, KIND) == "trace"
                                && matches!(field(b, EVENT), "tool-call" | "job-start" | "job")))
                })
            })
            .flat_map(|(id, events)| {
                std::iter::once(id.as_str())
                    .chain(events.iter().map(|e| e.hash.as_str()))
                    .chain(
                        events
                            .iter()
                            .filter(|e| field(e, KIND) == "cite" && field(e, CONTEXT) == ctx.0)
                            .map(|e| field(e, ORIGIN_ID)),
                    )
            })
            .collect()
    }

    /// Whether `id` is a kino some context cached from a repo ledger — the
    /// only store-held id another context may cache again for itself.
    fn cited_elsewhere(&self, id: &str) -> bool {
        self.by_id
            .values()
            .flatten()
            .any(|e| field(e, KIND) == "cite" && (e.id == id || field(e, ORIGIN_ID) == id))
    }

    /// Sort a submission's established lines by whether their evidence
    /// resolves: a kino id this store holds (or cached from a declared repo's
    /// ledger), or a URL.
    fn check_evidence(&self, ctx: &ContextId, submission: &Submission) -> Checked {
        let known = self.known(ctx);
        let mut checked = Checked::default();
        for line in &submission.established {
            let claim = one_line(&line.claim);
            if claim.is_empty() {
                continue;
            }
            let mut evidence: Vec<String> = Vec::new();
            for e in line.evidence.iter().map(|e| normalize(e)) {
                if !e.is_empty() && !evidence.contains(&e) {
                    evidence.push(e);
                }
            }
            let (good, bad): (Vec<String>, Vec<String>) = evidence
                .into_iter()
                .partition(|e| is_url(e) || known.contains(e.as_str()));
            for e in bad {
                if !checked.unresolved.contains(&e) {
                    checked.unresolved.push(e);
                }
            }
            if good.is_empty() {
                checked.downgraded.push(claim);
            } else {
                checked.established.push((claim, good));
            }
        }
        checked
    }

    /// The head versions nothing supersedes, newest first: one, unless two
    /// submits raced.
    fn tips(&self, ctx: &ContextId) -> Result<Vec<&Event>, Error> {
        self.require_context(ctx)?;
        Ok(self.tips_of(&ctx.0))
    }

    /// The versions of an identity nothing supersedes, newest first.
    fn tips_of(&self, id: &str) -> Vec<&Event> {
        let events = self.by_id.get(id).map(Vec::as_slice).unwrap_or_default();
        let referenced: std::collections::HashSet<&str> = events
            .iter()
            .flat_map(|e| e.parents.iter().map(String::as_str))
            .collect();
        let mut tips: Vec<&Event> = events
            .iter()
            .filter(|e| !referenced.contains(e.hash.as_str()))
            .collect();
        tips.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.hash.cmp(&b.hash)));
        tips.dedup_by(|a, b| a.hash == b.hash);
        tips
    }

    /// Claims on an item, as (claim birth event, its settle outcome if any).
    fn claims_on<'a>(&'a self, item: &str) -> Vec<(&'a Event, Option<&'a str>)> {
        let settles: HashMap<&str, &str> = self
            .by_id
            .values()
            .filter_map(|events| events.first())
            .filter(|e| field(e, KIND) == "settle")
            .map(|e| (field(e, CLAIM), field(e, OUTCOME)))
            .collect();
        self.by_id
            .values()
            .filter_map(|events| claim_birth(events))
            .filter(|e| field(e, ITEM) == item)
            .map(|e| (e, settles.get(e.id.as_str()).copied()))
            .collect()
    }

    fn validate_dependencies(
        &self,
        ctx: &ContextId,
        deps: &[ItemId],
        claimed: Option<&str>,
    ) -> Result<(), Error> {
        let mut seen = std::collections::HashSet::new();
        for dep in deps {
            if claimed == Some(dep.0.as_str()) {
                return Err(Error::InvalidDependency(
                    "an item cannot wait on itself".into(),
                ));
            }
            if !seen.insert(&dep.0) {
                return Err(Error::InvalidDependency(format!(
                    "{} is named more than once",
                    dep.0
                )));
            }
            let Some(event) = self.by_id.get(&dep.0).and_then(|events| events.first()) else {
                return Err(Error::InvalidDependency(format!("no item {}", dep.0)));
            };
            if field(event, KIND) != "item" || field(event, CONTEXT) != ctx.0 {
                return Err(Error::InvalidDependency(format!(
                    "{} is not an item in context {}",
                    dep.0, ctx.0
                )));
            }
        }
        Ok(())
    }

    fn waiting_on(&self, item: &Event) -> Vec<String> {
        let mut deps: Vec<String> = split_ids(field(item, AFTER))
            .into_iter()
            .map(str::to_owned)
            .collect();
        for event in self.by_id.values().filter_map(|events| events.first()) {
            if field(event, KIND) == "settle"
                && field(event, ITEM) == item.id
                && field(event, OUTCOME) == BLOCKED
            {
                deps.extend(
                    split_ids(field(event, BLOCKED_ON))
                        .into_iter()
                        .map(str::to_owned),
                );
            }
        }
        deps
    }

    fn dependencies_met(&self, item: &Event) -> bool {
        self.waiting_on(item).into_iter().all(|dep| {
            self.claims_on(&dep)
                .iter()
                .any(|(_, outcome)| *outcome == Some(SUBMITTED))
        })
    }

    /// The oldest ready item under `ctx` and the attempt its next claim takes.
    fn next_ready(&self, ctx: &ContextId, now: Now) -> Option<(&Event, u32)> {
        self.ready(ctx, now).into_iter().next()
    }

    /// Every ready item under `ctx`, oldest first, with the attempt its next
    /// claim takes.
    fn ready(&self, ctx: &ContextId, now: Now) -> Vec<(&Event, u32)> {
        let mut items: Vec<&Event> = self
            .of_kind_in("item", ctx)
            .into_iter()
            // Not before its time (kudo::queue::due-items).
            .filter(|item| due_of(item).is_none_or(|due| due <= now))
            .filter(|item| self.dependencies_met(item))
            .filter(|item| self.first_successor(item))
            .collect();
        // What came due first goes first (it was scheduled); then items with
        // no due time, in order.
        items.sort_by_key(|e| (due_of(e).is_none(), due_of(e).unwrap_or(0), seq_of(e)));
        items
            .into_iter()
            .filter_map(|item| {
                let claims = self.claims_on(&item.id);
                let submitted = claims
                    .iter()
                    .any(|(_, outcome)| *outcome == Some(SUBMITTED));
                let held = claims
                    .iter()
                    .any(|(claim, outcome)| outcome.is_none() && deadline_of(claim) > now);
                let attempt = u32::try_from(claims.len())
                    .unwrap_or(u32::MAX)
                    .saturating_add(1);
                (!submitted && !held).then_some((item, attempt))
            })
            .collect()
    }

    /// Whether `item` is the one successor of the check it was snoozed from:
    /// two machines snoozing the same check during a partition may each
    /// queue one, and the lowest hash stands for both.
    fn first_successor(&self, item: &Event) -> bool {
        let origin = field(item, SNOOZED_FROM);
        origin.is_empty()
            || self
                .by_id
                .values()
                .filter_map(|events| events.first())
                .filter(|e| field(e, KIND) == "item" && field(e, SNOOZED_FROM) == origin)
                .all(|e| e.hash >= item.hash)
    }

    /// The claim's birth event, if it is still live at `now`.
    fn live_claim(&self, claim: &ClaimId, now: Now) -> Result<&Event, Error> {
        let event = self
            .by_id
            .get(&claim.0)
            .and_then(|events| claim_birth(events))
            .ok_or_else(|| Error::UnknownClaim(claim.0.clone()))?;
        let settled = self
            .claims_on(field(event, ITEM))
            .iter()
            .any(|(c, outcome)| c.id == claim.0 && outcome.is_some());
        if settled {
            return Err(Error::ClaimSettled(claim.0.clone()));
        }
        if deadline_of(event) <= now {
            return Err(Error::ClaimExpired(claim.0.clone()));
        }
        Ok(event)
    }
}

/// A submission's established lines after the evidence check.
#[derive(Default)]
struct Checked {
    /// Lines kept as established, with the evidence that resolved.
    established: Vec<(String, Vec<String>)>,
    /// Lines moved to claimed: none of their evidence resolved.
    downgraded: Vec<String>,
    /// Evidence entries that did not resolve.
    unresolved: Vec<String>,
}

impl Checked {
    /// The kino ids cited as evidence that resolved (URLs left out).
    fn kinos(&self) -> Vec<&str> {
        let mut kinos: Vec<&str> = Vec::new();
        for e in self
            .established
            .iter()
            .flat_map(|(_, evidence)| evidence.iter().map(String::as_str))
        {
            if !is_url(e) && !kinos.contains(&e) {
                kinos.push(e);
            }
        }
        kinos
    }
}

/// A repo declaration that can stand: a name and a node with no spaces, `@`
/// or `:` (the declaration's own separators), and an absolute path.
fn validate(repo: &Repo) -> Result<(), Error> {
    for (what, word) in [("name", &repo.name), ("node", &repo.node)] {
        if word.is_empty()
            || word
                .chars()
                .any(|c| c.is_whitespace() || c == '@' || c == ':')
        {
            return Err(Error::InvalidRepo(format!(
                "the {what} {word:?} must be one word with no '@' or ':'"
            )));
        }
    }
    let Some(path) = repo.path.to_str() else {
        return Err(Error::InvalidRepo(format!(
            "the path {} is not UTF-8",
            repo.path.display()
        )));
    };
    if !repo.path.is_absolute() || path.contains('\n') {
        return Err(Error::InvalidRepo(format!(
            "the path {path:?} must be absolute"
        )));
    }
    Ok(())
}

/// Sixty-four hex digits: the shape of a kino id or version hash.
fn is_kino_id(evidence: &str) -> bool {
    evidence.len() == 64 && evidence.bytes().all(|b| b.is_ascii_hexdigit())
}

/// An http(s) URL with a host, and no whitespace anywhere.
fn is_url(evidence: &str) -> bool {
    let rest = evidence
        .strip_prefix("https://")
        .or_else(|| evidence.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/'))
        && !evidence.chars().any(char::is_whitespace)
}

/// An evidence entry as checked: trimmed, a `kino://` link reduced to its id,
/// an id lowercased.
fn normalize(evidence: &str) -> String {
    let e = evidence.trim();
    let e = e.strip_prefix("kino://").unwrap_or(e);
    if is_kino_id(e) {
        e.to_ascii_lowercase()
    } else {
        e.to_owned()
    }
}

/// Free text as one list item: internal line breaks collapse, so no item can
/// start a line of its own.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The worker's log as the body's lead: a line that would read as one of the
/// body's own markers (a heading, the head line) is escaped.
fn lead(log: &str) -> String {
    log.trim_end()
        .lines()
        .map(|line| {
            if line.starts_with('#') {
                format!("\\{line}")
            } else if line.starts_with("Head as left:") {
                format!(" {line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A submission's body, in fixed sections so it reads cold.
fn render(submission: &Submission, checked: &Checked, head_version: &str) -> String {
    fn list(out: &mut String, heading: &str, lines: &[String]) {
        out.push_str(&format!("\n## {heading}\n\n"));
        if lines.is_empty() {
            out.push_str("- none\n");
        }
        for line in lines {
            out.push_str(&format!("- {line}\n"));
        }
    }
    let mut out = format!("{}\n", lead(&submission.log));
    let established: Vec<String> = checked
        .established
        .iter()
        .map(|(claim, evidence)| format!("{claim} (evidence: {})", evidence.join(", ")))
        .collect();
    list(&mut out, "Established", &established);
    let claimed: Vec<String> = checked
        .downgraded
        .iter()
        .map(|c| format!("{c} (no evidence that resolved)"))
        .chain(submission.claimed.iter().map(|c| one_line(c)))
        .filter(|c| !c.is_empty())
        .collect();
    list(&mut out, "Claimed", &claimed);
    let ruled_out: Vec<String> = submission
        .ruled_out
        .iter()
        .map(|r| format!("{} — {}", one_line(&r.candidate), one_line(&r.because)))
        .collect();
    list(&mut out, "Ruled out", &ruled_out);
    let gaps: Vec<String> = submission
        .gaps
        .iter()
        .map(|g| one_line(g))
        .filter(|g| !g.is_empty())
        .collect();
    list(&mut out, "Gaps", &gaps);
    out.push_str(&format!("\nHead as left: version {head_version}\n"));
    out
}

/// Refuse a claim the given lease does not hold — judged on the same view
/// the write is made from, after the forced pull.
fn held_by_lease(claim_event: &Event, claim: &ClaimId, lease: Option<&str>) -> Result<(), Error> {
    match lease {
        Some(lease) if field(claim_event, LEASE) != lease => Err(Error::ClaimNotHeld {
            claim: claim.0.clone(),
            lease: lease.to_owned(),
        }),
        _ => Ok(()),
    }
}

fn birth(events: &[Event]) -> Option<&Event> {
    events.iter().find(|e| e.parents.is_empty())
}

/// The birth that holds a claim: racing pulls of one attempt each write a
/// birth with the same id, and the smallest lease wins — the same answer for
/// every reader, with no lock.
fn claim_birth(events: &[Event]) -> Option<&Event> {
    events
        .iter()
        .filter(|e| e.parents.is_empty() && field(e, KIND) == "claim")
        .min_by(|a, b| field(a, LEASE).cmp(field(b, LEASE)))
}

fn field<'a>(event: &'a Event, key: &str) -> &'a str {
    event
        .metadata
        .get(key)
        .map(String::as_str)
        .unwrap_or_default()
}

/// How much of a shell call's output a trace keeps: this much from each end.
const OUTPUT_KEPT: usize = 2048;

/// The first and last `keep` bytes of `text` (on character boundaries), the
/// middle cut and said so.
fn excerpt(text: &str, keep: usize) -> String {
    if text.len() <= 2 * keep {
        return text.to_owned();
    }
    let mut head = keep;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - keep;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n… ({} bytes cut) …\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

fn split_ids(text: &str) -> Vec<&str> {
    text.split(',').filter(|id| !id.is_empty()).collect()
}

/// An item's due time, if it has one.
fn due_of(event: &Event) -> Option<Now> {
    field(event, DUE).parse().ok()
}

fn seq_of(event: &Event) -> u64 {
    field(event, SEQ).parse().unwrap_or(0)
}

/// The head versions a claim was taken from.
fn base_of(event: &Event) -> Vec<String> {
    field(event, BASE)
        .split(',')
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

fn deadline_of(event: &Event) -> Now {
    field(event, DEADLINE).parse().unwrap_or(0)
}

/// A head version's bytes: the body under a marker naming what wrote it, so
/// no two versions share a hash.
fn head_content(writer: &str, body: &str) -> Vec<u8> {
    format!("<!-- kudo head {writer} -->\n{body}").into_bytes()
}

/// Drop the one-line `<!-- kudo … -->` marker that makes a kino unique.
fn strip_marker(content: &str) -> String {
    match content.split_once('\n') {
        Some((first, rest)) if first.starts_with("<!-- kudo ") => rest.to_owned(),
        _ => content.to_owned(),
    }
}

/// Sixteen random bytes as hex, from the kernel.
fn nonce() -> Result<String, Error> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn meta<const N: usize>(pairs: [(&str, &str); N]) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn ts(now: Now) -> String {
    jiff::Timestamp::from_second(now)
        .map(|t| t.to_string())
        .unwrap_or_else(|_| now.to_string())
}

fn kin(e: impl std::fmt::Display) -> Error {
    Error::Kinora(e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    const T0: Now = 1_792_500_000;

    /// A racing pull wrote the same attempt with another lease. Exactly one of
    /// the two leases holds the claim, and it is the same one for every reader.
    #[test]
    fn of_two_racing_claims_on_one_attempt_exactly_one_holds() {
        let dir = std::env::temp_dir().join(format!("kudo-context-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("race", "h\n", T0).unwrap();
        let item = store.enqueue(&ctx, "contended", T0).unwrap();
        let claim = store.pull(&ctx, "lease-b", T0, 600).unwrap().unwrap();

        // The other writer's birth of the same claim, with a smaller lease.
        let content = format!("kudo claim\nitem: {}\nattempt: 1\n", item.0);
        let deadline = (T0 + 600).to_string();
        store
            .put(
                "text",
                content.into_bytes(),
                meta([
                    (KIND, "claim"),
                    (CONTEXT, &ctx.0),
                    (ITEM, &item.0),
                    (ATTEMPT, "1"),
                    (LEASE, "lease-a"),
                    (DEADLINE, &deadline),
                ]),
                None,
                T0,
            )
            .unwrap();

        assert!(store.confirm(&claim.id, "lease-a").unwrap());
        assert!(!store.confirm(&claim.id, "lease-b").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two submits raced past the moved-head check: the head forked. A claim
    /// taken now builds on both tips, and its submit closes the fork.
    // implements: 3a472b4021f7b7496dd37779f3d0c76cafe1c95e8e80b1b236786a81e4c7d0ab@aaddfc2e7ee067ea5780ce6ee2dfe4587bfb47b0c067b2670a31308c019a7e21
    #[test]
    fn a_forked_head_closes_on_the_next_submit() {
        let dir = std::env::temp_dir().join(format!("kudo-context-fork-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("fork", "h0\n", T0).unwrap();
        let root = store.head(&ctx).unwrap().version;
        for (body, at) in [("left\n", T0 + 1), ("right\n", T0 + 2)] {
            store
                .put(
                    "markdown",
                    body.as_bytes().to_vec(),
                    meta([(KIND, "head")]),
                    Some((ctx.0.clone(), vec![root.clone()])),
                    at,
                )
                .unwrap();
        }
        let heads = store.heads(&ctx).unwrap();
        assert_eq!(heads.len(), 2, "{heads:?}");
        assert_eq!(heads[0].body, "right\n", "newest first");
        assert_eq!(store.head(&ctx).unwrap().body, "right\n");

        store.enqueue(&ctx, "merge", T0 + 3).unwrap();
        let claim = store.pull(&ctx, "lease-a", T0 + 3, 600).unwrap().unwrap();
        let mut base = claim.base.clone();
        base.sort();
        let mut tips: Vec<String> = heads.iter().map(|h| h.version.clone()).collect();
        tips.sort();
        assert_eq!(base, tips);

        let merged = Submission {
            head: "left\nright\n".into(),
            log: "merged".into(),
            ..Default::default()
        };
        store.submit(&claim.id, &merged, T0 + 4).unwrap();
        let heads = store.heads(&ctx).unwrap();
        assert_eq!(heads.len(), 1, "the fork is closed: {heads:?}");
        assert_eq!(heads[0].body, "left\nright\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fork closes even when the worker keeps one tip's bytes as they are.
    #[test]
    fn a_fork_closes_on_a_submit_that_keeps_one_tip() {
        let dir =
            std::env::temp_dir().join(format!("kudo-context-fork-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("fork", "h0\n", T0).unwrap();
        let root = store.head(&ctx).unwrap().version;
        for (body, at) in [("older\n", T0 + 1), ("newer\n", T0 + 2)] {
            store
                .put(
                    "markdown",
                    body.as_bytes().to_vec(),
                    meta([(KIND, "head")]),
                    Some((ctx.0.clone(), vec![root.clone()])),
                    at,
                )
                .unwrap();
        }
        store.enqueue(&ctx, "pick one", T0 + 3).unwrap();
        let claim = store.pull(&ctx, "lease-a", T0 + 3, 600).unwrap().unwrap();
        let keep = Submission {
            head: "older\n".into(),
            log: "the older tip was right".into(),
            ..Default::default()
        };
        store.submit(&claim.id, &keep, T0 + 4).unwrap();
        let heads = store.heads(&ctx).unwrap();
        assert_eq!(heads.len(), 1, "{heads:?}");
        assert_eq!(heads[0].body, "older\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two declarations raced and forked the repos kino: both entries show,
    /// and the next declaration keeps them both.
    // implements: b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9@b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9
    #[test]
    fn a_forked_repos_declaration_loses_no_repo() {
        let dir =
            std::env::temp_dir().join(format!("kudo-context-repos-fork-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("fork", "h\n", T0).unwrap();
        let repo = |name: &str| Repo {
            name: name.into(),
            node: "n".into(),
            path: PathBuf::from(format!("/w/{name}")),
        };
        store.declare_repo(&ctx, &repo("a"), T0).unwrap();
        // A sibling of that version, as a racing declarer would write it.
        let view = store.view().unwrap();
        let birth = format!("<!-- kudo repos {} -->\n# Repos\n", ctx.0);
        let id = Hash::of_content(birth.as_bytes()).as_hex().to_owned();
        let root = birth_hash(&view, &id);
        store
            .put(
                "markdown",
                format!(
                    "<!-- kudo repos {} x -->\n# Repos\n\n- b @ n: /w/b\n",
                    ctx.0
                )
                .into_bytes(),
                meta([(KIND, "repos"), (CONTEXT, &ctx.0)]),
                Some((id, vec![root])),
                T0 + 1,
            )
            .unwrap();
        let names = |s: &ContextStore| -> Vec<String> {
            s.repos(&ctx).unwrap().into_iter().map(|r| r.name).collect()
        };
        assert_eq!(names(&store), ["a", "b"]);
        store.declare_repo(&ctx, &repo("c"), T0 + 2).unwrap();
        assert_eq!(names(&store), ["a", "b", "c"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn birth_hash(view: &View, id: &str) -> String {
        view.by_id
            .get(id)
            .and_then(|events| birth(events))
            .map(|e| e.hash.clone())
            .unwrap()
    }

    /// A claim taken before claims recorded their base is checked against
    /// nothing: its first submit is not refused for a move nobody made.
    #[test]
    fn a_claim_without_a_recorded_base_is_not_refused() {
        let dir = std::env::temp_dir().join(format!("kudo-context-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("legacy", "h\n", T0).unwrap();
        let item = store.enqueue(&ctx, "old claim", T0).unwrap();
        let content = format!("kudo claim\nitem: {}\nattempt: 1\n", item.0);
        let deadline = (T0 + 600).to_string();
        let born = store
            .put(
                "text",
                content.into_bytes(),
                meta([
                    (KIND, "claim"),
                    (CONTEXT, &ctx.0),
                    (ITEM, &item.0),
                    (ATTEMPT, "1"),
                    (LEASE, "lease-a"),
                    (DEADLINE, &deadline),
                ]),
                None,
                T0,
            )
            .unwrap();
        let sub = Submission {
            head: "h2\n".into(),
            log: "done".into(),
            ..Default::default()
        };
        store.submit(&ClaimId(born.id), &sub, T0 + 1).unwrap();
        assert_eq!(store.head(&ctx).unwrap().body, "h2\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
