//! The MCP surface the agent sees.
//!
//! Every tool here is a **proxy**: it asks the mesh and returns the answer. No
//! tool computes something an engine could have answered, and none caches — two
//! readers must never see different worlds.
//!
//! The surface is deliberately small. Agent tool surface is a scarce resource,
//! and near-identical tools are a real cost rather than clutter: each one is a
//! chance to pick the wrong one
//! (`kudo:a9a94ad3464ebf50c30bffcfe64d4596384d5d122c5dfb3fb992b1aa73cd7eba`).
//!
//! implements: 121ac6ebe48b717b93e775f5a0526076a9230ec0e10e748dbcbaf181bf758120

use std::sync::Arc;

use moco_job::wire::{
    ClearRequest, EnsureRequest, KillRequest, MachineRequest, RestartRequest, ScreenRequest,
    StartNamedRequest, StartRequest, StatsRequest, TailRequest,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::schemars;
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::hub::HubClient;
use crate::jobs::{self, DEFAULT_JOB_CONNECTOR};
use crate::mesh::render_mesh;
use crate::route::Target;
use crate::worker_tools;
use kudo_context::Now;
use kudo_worker::WorkerSetup;

/// Seconds since the Unix epoch.
fn now() -> Now {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Which node (and optionally which link) a call is addressed to.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct NodeArgs {
    /// The node's registry name, as shown by `nodes`.
    pub node: String,
    /// Which link of that node. Omit for the node's default (`stable`); the hub
    /// falls back to `stable` if the named link is not connected.
    pub link: Option<String>,
    /// The job connector's id on that node. Omit for the conventional default.
    pub connector: Option<String>,
}

/// Start a job this workspace declares.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct StartNamedArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    /// The name the workspace's manifest declares it under — or, with
    /// `node_recipe`, one of the node's own recipes (see `recipes`).
    pub name: String,
    /// The name is one of the node's own recipes, startable from any host:
    /// list them with `recipes`. Omit for this workspace's manifest.
    #[serde(default)]
    pub node_recipe: bool,
}

/// Restart a declared job.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct RestartArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    /// The job id, from job_list.
    pub id: String,
}

/// Start a job ad-hoc.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct StartArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    /// The command as an **argument vector** — `["cargo", "test"]`, never a
    /// shell string. Shell metacharacters are inert data, so quoting tricks do
    /// nothing; a command needing a pipeline ships a wrapper program as its
    /// first element.
    pub argv: Vec<String>,
    /// Working directory. Omit to use this session's own directory on this
    /// machine's node, or the node's allowed root on any other node.
    pub cwd: Option<String>,
    /// Give up after this many milliseconds. Omit for no deadline.
    pub timeout_ms: Option<u64>,
}

/// Read a job's output.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct TailArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    /// The job id returned by `job_start`.
    pub id: String,
    /// Resume from this byte offset; use the `next_offset` from the previous
    /// read to get only what is new. Omit to read from the beginning.
    pub offset: Option<u64>,
}

/// Wait for a job to end.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct WaitArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    /// The job id returned by `job_start_adhoc`.
    pub id: String,
    /// Wait at most this many seconds (default 120, at most 1500); a job still
    /// running or still waiting for a verdict then says so, and you can wait
    /// again. Keep it under your client's tool-call timeout.
    pub timeout_s: Option<u64>,
}

/// Terminate a job.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct KillArgs {
    pub node: String,
    pub link: Option<String>,
    pub connector: Option<String>,
    pub id: String,
}

/// Submit the work for a claim.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct SubmitArgs {
    /// The claim id, from `pull` or the item you were handed.
    pub claim: String,
    /// The **full** new head, as you leave it: where it stands (verdict
    /// first), the next action and who takes it, what is established with its
    /// evidence, and what is still open. It replaces the old head whole.
    pub head: String,
    /// A few lines on what you did, in your own words.
    pub log: String,
    /// What you established, each with its evidence: kino ids or http(s)
    /// URLs. A line none of whose evidence resolves lands as claimed.
    #[serde(default)]
    pub established: Vec<EstablishedArg>,
    /// What you believe but cannot back with evidence.
    #[serde(default)]
    pub claimed: Vec<String>,
    /// What you ruled out, and why.
    #[serde(default)]
    pub ruled_out: Vec<RuledOutArg>,
    /// What you could not see or check.
    #[serde(default)]
    pub gaps: Vec<String>,
    /// Only after a refused submit: the head version(s) the refusal gave you,
    /// which your new head builds on.
    #[serde(default)]
    pub base: Option<String>,
    /// Only for a check: `pass`, `fail` or `snooze`.
    #[serde(default)]
    pub verdict: Option<String>,
    /// With `snooze`: when to look again — `+30m`, `+2h`, `+1d`, or an RFC
    /// 3339 time.
    #[serde(default)]
    pub snooze_until: Option<String>,
    /// With a verdict: what you saw, in a line.
    #[serde(default)]
    pub note: Option<String>,
    /// Wait instead of finishing: existing items in this context that must submit first.
    #[serde(default)]
    pub blocked_on: Vec<String>,
}

/// One established line.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct EstablishedArg {
    /// The fact, in one line.
    pub claim: String,
    /// What backs it: kino ids or http(s) URLs.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// One candidate ruled out.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct RuledOutArg {
    pub candidate: String,
    pub because: String,
}

impl SubmitArgs {
    /// The typed submission this call hands the engine, or why its verdict
    /// cannot stand.
    fn submission(&self, now: kudo_context::Now) -> Result<kudo_context::Submission, String> {
        Ok(kudo_context::Submission {
            head: self.head.clone(),
            log: self.log.clone(),
            established: self
                .established
                .iter()
                .map(|e| kudo_context::Established {
                    claim: e.claim.clone(),
                    evidence: e.evidence.clone(),
                })
                .collect(),
            claimed: self.claimed.clone(),
            ruled_out: self
                .ruled_out
                .iter()
                .map(|r| kudo_context::RuledOut {
                    candidate: r.candidate.clone(),
                    because: r.because.clone(),
                })
                .collect(),
            gaps: self.gaps.clone(),
            base: self.base.as_deref().map(split_versions),
            check: worker_tools::check_close(
                self.verdict.as_deref(),
                self.snooze_until.as_deref(),
                self.note.as_deref(),
                now,
            )?,
            blocked_on: self
                .blocked_on
                .iter()
                .map(|id| kudo_context::ItemId(id.clone()))
                .collect(),
        })
    }
}

/// Head versions as a refusal gives them: comma-separated.
fn split_versions(base: &str) -> Vec<String> {
    base.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// File a rough edge.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct InboxArgs {
    /// The rough edge in one line: what got in the way.
    pub line: String,
    /// More detail, if one line is not enough.
    pub body: Option<String>,
}

/// Give a claim back.
#[derive(Deserialize, schemars::JsonSchema, Debug)]
pub struct ReleaseArgs {
    /// The claim id.
    pub claim: String,
}

fn target(node: &str, link: &Option<String>) -> Target {
    match link {
        Some(link) => Target::link(node, link),
        None => Target::node(node),
    }
}

fn connector_id(connector: &Option<String>) -> String {
    connector
        .clone()
        .unwrap_or_else(|| DEFAULT_JOB_CONNECTOR.to_string())
}

/// Turn a result into a tool reply, keeping a failure a *failure*.
///
/// A refusal rendered as ordinary output would read as success to an agent,
/// which is how a caller ends up believing work happened.
fn reply(result: Result<String, crate::ShimError>) -> Result<CallToolResult, ErrorData> {
    Ok(match result {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
    })
}

/// The agent-facing server.
///
/// How long a lazy dial of the hub may take before a job tool gives up.
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The tools that work against the context store, with or without a hub.
const WORKER_TOOLS: [&str; 6] = [
    "context_read",
    "pull",
    "submit",
    "release",
    "inbox",
    "evidence",
];

/// Where the hub stands for this session.
enum HubState {
    /// Dialled before serving (a session that is only job tools).
    Dialled(Arc<HubClient>),
    /// Dialled when a job tool first needs it, and again after a failure (a
    /// worker session: a hub that is down at start may be up later).
    Lazy {
        addr: String,
        timeout: Option<std::time::Duration>,
        dialled: tokio::sync::Mutex<Option<Arc<HubClient>>>,
    },
    /// No hub address: the job tools are not offered; why.
    Absent(String),
}

/// Holds a hub connection and nothing else — no job state, no mesh cache.
#[derive(Clone)]
pub struct ShimServer {
    /// The hub, or why there is none (the job tools say so; the worker
    /// tools do not need it).
    /// Shared by every clone, so a hub dialled once serves the session.
    hub: Arc<HubState>,
    /// Set when this session is a worker (the launcher's environment says so).
    worker: Option<WorkerSetup>,
}

#[tool_router]
impl ShimServer {
    /// Build a server with no hub: the worker tools work, each job tool
    /// answers that there is no hub, and `why`.
    // implements: 482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae@482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae
    pub fn without_hub(why: String) -> Self {
        Self {
            hub: Arc::new(HubState::Absent(why)),
            worker: None,
        }
    }

    /// A server that dials the hub at `addr` when a job tool first needs it,
    /// and again after a failure.
    // implements: 482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae@482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae
    pub fn lazy(addr: String, timeout: Option<std::time::Duration>) -> Self {
        Self {
            hub: Arc::new(HubState::Lazy {
                addr,
                timeout,
                dialled: tokio::sync::Mutex::new(None),
            }),
            worker: None,
        }
    }

    /// The names of the tools this session lists.
    pub fn listed_tool_names(&self) -> Vec<String> {
        self.listed_tools()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    /// The tools this session lists: with no hub address at all, only the
    /// worker tools — the others could never work here.
    fn listed_tools(&self) -> Vec<rmcp::model::Tool> {
        let all = Self::tool_router().list_all();
        match self.hub.as_ref() {
            HubState::Absent(_) => all
                .into_iter()
                .filter(|t| WORKER_TOOLS.contains(&t.name.as_ref()))
                .collect(),
            _ => all,
        }
    }

    /// The `nodes` tool, called directly (for tests and other faces).
    pub async fn call_nodes(&self) -> CallToolResult {
        self.nodes()
            .await
            .unwrap_or_else(|e| CallToolResult::error(vec![ContentBlock::text(e.message)]))
    }

    /// The hub, or the tool error a job tool answers without one.
    async fn need_hub(&self) -> Result<Arc<HubClient>, CallToolResult> {
        let no_hub = |why: String| {
            CallToolResult::error(vec![ContentBlock::text(format!(
                "no hub: {why}. This session's worker tools (context_read, pull, submit, \
                 release, inbox, evidence) work without one; the job and node tools need a hub."
            ))])
        };
        match self.hub.as_ref() {
            HubState::Dialled(hub) => Ok(hub.clone()),
            HubState::Absent(why) => Err(no_hub(why.clone())),
            HubState::Lazy {
                addr,
                timeout,
                dialled,
            } => {
                let mut held = dialled.lock().await;
                if let Some(hub) = held.as_ref() {
                    return Ok(hub.clone());
                }
                match tokio::time::timeout(DIAL_TIMEOUT, HubClient::dial(addr)).await {
                    Ok(Ok(hub)) => {
                        let hub = Arc::new(match timeout {
                            Some(t) => hub.with_timeout(*t),
                            None => hub,
                        });
                        *held = Some(hub.clone());
                        Ok(hub)
                    }
                    Ok(Err(e)) => Err(no_hub(format!(
                        "the hub did not answer ({e}); it is tried again on the next call"
                    ))),
                    Err(_) => Err(no_hub(format!(
                        "the hub at {addr} did not answer within {}s; it is tried again on \
                         the next call",
                        DIAL_TIMEOUT.as_secs()
                    ))),
                }
            }
        }
    }

    /// Build a server over an already-dialed hub connection.
    pub fn new(hub: Arc<HubClient>) -> Self {
        Self {
            hub: Arc::new(HubState::Dialled(hub)),
            worker: None,
        }
    }

    /// Trace the end of a job this worker started (once; best effort), with
    /// what it printed; what to tell the worker to cite, when recorded.
    // implements: e23a8b06aa9331b3f1ffd8df36be1392daf44e93ca20037b50cec6c3132f53e1@d9656cc142296b3c9017b9d73ca59d326c9a29abd2945a5afd4a73eb6514f593
    async fn trace_end(
        &self,
        hub: &HubClient,
        target: crate::route::Target,
        connector: &str,
        node: &str,
        id: &str,
        end: &moco_job::wire::WaitReply,
    ) -> String {
        let Some(setup) = &self.worker else {
            return String::new();
        };
        if !worker_tools::awaits_end(setup, id).unwrap_or(false) {
            return String::new();
        }
        // What the job printed, as the node keeps it. Output that cannot be
        // read is said so in the record (the end is still traced), and the
        // worker is not told it holds output.
        let read = jobs::tail(
            hub,
            target,
            connector,
            TailRequest {
                id: id.to_owned(),
                offset: 0,
            },
        )
        .await;
        let (output, readable) = match &read {
            Ok(t) => (worker_tools::output_text(&t.bytes, t.skipped), true),
            Err(e) => (format!("(output unreadable: {e})"), false),
        };
        match worker_tools::trace_job_end(
            setup,
            node,
            id,
            &format!(
                "{}{}",
                jobs::status_word(&end.status),
                jobs::end_note(&end.status)
            ),
            &jobs::verdict_word(end.verdict),
            &output,
            now(),
        ) {
            Ok(Some(kino)) if readable => {
                format!(" (recorded with its output as kino://{kino}; cite it)")
            }
            Ok(Some(kino)) => format!(
                " (its end is recorded as kino://{kino}, but its output could not be read: \
                 read it with job_tail; a claim from it stays claimed)"
            ),
            Ok(None) => String::new(),
            Err(e) => {
                eprintln!("kudo: could not trace the end of job {id}: {e}");
                String::new()
            }
        }
    }

    /// The same server, answering the worker tools for `setup`.
    pub fn with_worker(mut self, setup: Option<WorkerSetup>) -> Self {
        self.worker = setup;
        self
    }

    fn worker_call(
        &self,
        f: impl FnOnce(&WorkerSetup, Now) -> Result<String, String>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = match &self.worker {
            Some(setup) => f(setup, now()),
            None => Err(
                "this session is not a kudo worker: it was not started under a \
                         context (KUDO_CONTEXT is unset)"
                    .to_string(),
            ),
        };
        Ok(match result {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        })
    }

    #[tool(
        name = "context_read",
        description = "Read the head of the context this worker is bound to: where the work \
                       stands, the next action, what is established and what is open."
    )]
    async fn context_read(&self) -> Result<CallToolResult, ErrorData> {
        self.worker_call(|s, _| worker_tools::context_read(s))
    }

    #[tool(
        name = "evidence",
        description = "List this lease's recorded shell calls with the kino ids to cite as evidence. Use after a shell call when the harness does not deliver hook context."
    )]
    async fn evidence(&self) -> Result<CallToolResult, ErrorData> {
        self.worker_call(|s, _| worker_tools::evidence(s))
    }

    #[tool(
        name = "pull",
        description = "Claim the next item queued under this worker's context. Returns the \
                       ask and a claim id; work it, then `submit` with that id. Says so when \
                       nothing is ready."
    )]
    async fn pull(&self) -> Result<CallToolResult, ErrorData> {
        self.worker_call(worker_tools::pull)
    }

    #[tool(
        name = "submit",
        description = "Hand back the work for a claim: the full new head of the context, a \
                       short log, and what you established (each line with evidence: kino \
                       ids or http(s) URLs), claimed, ruled out and could not see. A line \
                       whose evidence does not resolve lands as claimed. The head is \
                       superseded, never edited; the claim is settled. A check's claim also \
                       takes its verdict (pass, fail, or snooze with snooze_until) and a note. \
                       To wait instead of finishing, set blocked_on to existing item ids: the \
                       attempt is logged, the head stays unchanged, and the item waits for them."
    )]
    async fn submit(
        &self,
        Parameters(args): Parameters<SubmitArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.worker_call(|s, now| worker_tools::submit(s, &args.claim, &args.submission(now)?, now))
    }

    #[tool(
        name = "inbox",
        description = "File a rough edge you hit while working — a tool that misbehaved, a \
                       missing capability, an instruction that did not fit — in one line, \
                       with an optional body. It lands in this context's inbox for triage; \
                       it does not change the work."
    )]
    async fn inbox(
        &self,
        Parameters(args): Parameters<InboxArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.worker_call(|s, now| worker_tools::inbox(s, &args.line, args.body.as_deref(), now))
    }

    #[tool(
        name = "release",
        description = "Give a claim back unfinished; its item becomes ready again for the next \
                       pull. Use this rather than leaving a claim to expire."
    )]
    async fn release(
        &self,
        Parameters(args): Parameters<ReleaseArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.worker_call(|s, now| worker_tools::release(s, &args.claim, now))
    }

    #[tool(
        name = "nodes",
        description = "List the machines currently reachable through the hub, each link they \
                       have connected, and the capabilities (connectors) that link offers. Use \
                       this to find out what exists before addressing work to a node."
    )]
    async fn nodes(&self) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        // No arguments: reads are link-agnostic, so there is no link to select
        // and nothing for the caller to get wrong.
        match hub.topology().await {
            Ok(snapshot) => Ok(CallToolResult::success(vec![ContentBlock::text(
                render_mesh(&snapshot),
            )])),
            // Surface the failure as a tool error rather than as empty output:
            // "nothing is connected" and "the hub did not answer" must never
            // look alike to the agent.
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(
                e.to_string(),
            )])),
        }
    }

    #[tool(
        name = "job_list",
        description = "List the jobs a node's substrate knows about, with each one's state. \
                       Jobs outlive the session that started them, so work started earlier \
                       shows up here too."
    )]
    async fn job_list(
        &self,
        Parameters(args): Parameters<NodeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::list(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
            )
            .await
            .map(|r| jobs::render_jobs(&r)),
        )
    }

    #[tool(
        name = "recipes",
        description = "List a node's own recipes: named commands its manifest declares, each \
                       with what it is for, what it runs and where. Start one with job_start, \
                       node_recipe true — from any host, without knowing its exact argv."
    )]
    async fn recipes(
        &self,
        Parameters(args): Parameters<NodeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::recipes(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
            )
            .await
            .map(|r| jobs::render_recipes(&r)),
        )
    }

    #[tool(
        name = "job_start",
        description = "Start a job this workspace declares in its manifest, by name — or, \
                       with node_recipe, one of the node's own recipes (see `recipes`), from \
                       any host. Prefer this over job_start_adhoc: a declaration is \
                       reproducible, is reviewed in version control, and carries its own \
                       lifetime, restart policy and port. The manifest is re-read on every \
                       start, so an edit takes effect immediately. Either way the node's gate \
                       decides, as for any start."
    )]
    async fn job_start(
        &self,
        Parameters(args): Parameters<StartNamedArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::start_named(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                StartNamedRequest {
                    node_recipe: args.node_recipe,
                    name: args.name,
                    caller: jobs::session_caller(),
                },
            )
            .await
            .map(|r| format!("started job {}", r.id)),
        )
    }

    #[tool(
        name = "job_restart",
        description = "Stop a declared job and start it again from its *current* declaration. \
                       The manifest is re-read and re-authorized, so an edit takes effect and \
                       a newly-disallowed command is refused. Ad-hoc jobs cannot be restarted \
                       — there is no declaration to re-read."
    )]
    async fn job_restart(
        &self,
        Parameters(args): Parameters<RestartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::restart(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                RestartRequest {
                    id: args.id,
                    caller: jobs::session_caller(),
                },
            )
            .await
            .map(|r| format!("restarted as job {}", r.id)),
        )
    }

    #[tool(
        name = "job_ensure",
        description = "Start this workspace's session-autostart jobs that are not already \
                       running. Idempotent and safe to re-run: it starts what is missing and \
                       never stops or restarts anything. Run it once at the start of a session."
    )]
    async fn job_ensure(
        &self,
        Parameters(args): Parameters<NodeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::ensure(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                EnsureRequest {
                    caller: jobs::session_caller(),
                },
            )
            .await
            .map(|r| {
                if r.started.is_empty() {
                    "everything declared for this session is already running".to_string()
                } else {
                    format!("started: {}", r.started.join(", "))
                }
            }),
        )
    }

    #[tool(
        name = "job_clear",
        description = "Forget this workspace's finished jobs. Only finished ones — a running \
                       job is left strictly alone, so this can never stop anything. Their \
                       output goes with them, so read anything you still need first."
    )]
    async fn job_clear(
        &self,
        Parameters(args): Parameters<NodeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::clear(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                ClearRequest {
                    caller: jobs::session_caller(),
                },
            )
            .await
            .map(|r| format!("cleared {} finished job(s)", r.removed)),
        )
    }

    #[tool(
        name = "job_start_adhoc",
        description = "Start an arbitrary command as a job, without a declaration. Use \
                       job_start instead when the workspace declares what you want — this is \
                       for one-off work. Ad-hoc jobs cannot be restarted."
    )]
    async fn job_start_adhoc(
        &self,
        Parameters(args): Parameters<StartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        // The working directory defaults to **this shim's own** on its own
        // node — the session's project directory, inherited for free because
        // the shim runs as a child of the session — and to the node's allowed
        // root anywhere else, where this path does not exist.
        let cwd = match args.cwd {
            Some(cwd) => cwd,
            None => match jobs::adhoc_cwd(
                &args.node,
                &crate::cli::node_name(),
                std::env::current_dir(),
            ) {
                Ok(cwd) => cwd,
                Err(e) => return reply(Err(e)),
            },
        };

        let argv = args.argv.clone();
        let started = jobs::start_gated(
            &hub,
            target(&args.node, &args.link),
            &connector_id(&args.connector),
            StartRequest {
                argv: args.argv,
                cwd,
                deadline_ms: args.timeout_ms.unwrap_or(0),
                caller: jobs::session_caller(),
                env: vec![],
                human_view: moco_job::lens::HumanView::Logs,
            },
        )
        .await;
        // A worker traces the jobs it starts, so its context says what ran on
        // which node under which authority; the trace kino is citable. Best
        // effort: the job is started.
        let mut cite = String::new();
        if let (Ok(s), Some(setup)) = (&started, &self.worker) {
            match worker_tools::trace_job_start(setup, &args.node, &s.id, s.waiting, &argv, now()) {
                Ok(id) => cite = format!(" (recorded as kino://{id})"),
                Err(e) => eprintln!("kudo: could not trace job {}: {e}", s.id),
            }
        }
        // Refused at the start (a deny rule), or done already: say how it
        // ended and under which authority, not that it started.
        let ended = match &started {
            Ok(s) if s.status.is_terminal() => jobs::wait_for(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                &s.id,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            )
            .await
            .ok()
            .flatten(),
            _ => None,
        };
        let mut cite_end = String::new();
        if let (Ok(s), Some(end)) = (&started, &ended) {
            cite_end = self
                .trace_end(
                    &hub,
                    target(&args.node, &args.link),
                    &connector_id(&args.connector),
                    &args.node,
                    &s.id,
                    end,
                )
                .await;
        }
        reply(started.map(|s| {
            if let Some(end) = &ended {
                format!(
                    "job {} ended {}{} under {}{cite_end}",
                    s.id,
                    jobs::status_word(&end.status),
                    jobs::end_note(&end.status),
                    jobs::verdict_word(end.verdict)
                )
            } else if s.waiting {
                format!(
                    "job {} is waiting for the author's verdict on node {}{cite}: no rule \
                     allows this exact argv there. job_wait it — it runs once approved, or \
                     comes back denied (an unanswered request is denied after the node's \
                     timeout, 30 minutes by default).",
                    s.id, args.node
                )
            } else {
                format!("started job {}{cite}", s.id)
            }
        }))
    }

    #[tool(
        name = "job_wait",
        description = "Wait for a job to end — including one waiting for the author's verdict — \
                       and say how it ended and under which authority (a rule, the author's \
                       approval, or a denial). Comes back after timeout_s (default 120) if it \
                       has not ended yet; wait again. Read its output with job_tail."
    )]
    async fn job_wait(
        &self,
        Parameters(args): Parameters<WaitArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        let budget = std::time::Duration::from_secs(args.timeout_s.unwrap_or(120).min(1500));
        let ended = jobs::wait_for(
            &hub,
            target(&args.node, &args.link),
            &connector_id(&args.connector),
            &args.id,
            budget,
            std::time::Duration::from_secs(2),
        )
        .await;
        let mut cite = String::new();
        if let Ok(Some(end)) = &ended {
            cite = self
                .trace_end(
                    &hub,
                    target(&args.node, &args.link),
                    &connector_id(&args.connector),
                    &args.node,
                    &args.id,
                    end,
                )
                .await;
        }
        reply(ended.map(|end| match end {
            Some(end) => format!(
                "job {} ended {}{} under {}{cite}",
                args.id,
                jobs::status_word(&end.status),
                jobs::end_note(&end.status),
                jobs::verdict_word(end.verdict)
            ),
            None => format!(
                "job {} has not ended after {}s (still running, or still waiting for a \
                 verdict); wait again",
                args.id,
                budget.as_secs()
            ),
        }))
    }

    #[tool(
        name = "job_tail",
        description = "Read a job's output and its current state. Pass the previous reply's \
                       next_offset to get only what is new. This is also how you learn a job \
                       finished — the state comes back with every read."
    )]
    async fn job_tail(
        &self,
        Parameters(args): Parameters<TailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        let tailed = jobs::tail(
            &hub,
            target(&args.node, &args.link),
            &connector_id(&args.connector),
            TailRequest {
                id: args.id.clone(),
                offset: args.offset.unwrap_or(0),
            },
        )
        .await;
        // A worker that sees its job end here traces the end, as job_wait does
        // — only for a job it started whose end is not traced yet, so a
        // finished job's every read costs no extra calls.
        let mut cite = String::new();
        if let Some(setup) = &self.worker
            && let Ok(t) = &tailed
            && t.status.is_terminal()
            && worker_tools::awaits_end(setup, &args.id).unwrap_or(false)
            && let Ok(Some(end)) = jobs::wait_for(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                &args.id,
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            )
            .await
        {
            cite = self
                .trace_end(
                    &hub,
                    target(&args.node, &args.link),
                    &connector_id(&args.connector),
                    &args.node,
                    &args.id,
                    &end,
                )
                .await;
        }
        reply(tailed.map(|r| format!("{}{cite}", jobs::render_tail(&r))))
    }

    #[tool(
        name = "job_read",
        description = "Read a job's findings through its machine view — the structured sidecar \
                       it declares, if it declares one. **Prefer this over job_tail**: it is a \
                       few hundred bytes of answer instead of the whole terminal stream. The \
                       reply says which channel it came from, and falls back to scrollback for \
                       a job that declares no machine view."
    )]
    async fn job_read(
        &self,
        Parameters(args): Parameters<TailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::machine(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                MachineRequest {
                    id: args.id,
                    offset: args.offset.unwrap_or(0),
                },
            )
            .await
            .map(|r| jobs::render_machine(&r)),
        )
    }

    #[tool(
        name = "job_screen",
        description = "What this job's terminal looks like right now — the visible screen, not \
                       its history. Use this for anything that redraws (progress bars, TUIs, \
                       test runners): job_tail would give you every superseded frame, this \
                       gives you the current one."
    )]
    async fn job_screen(
        &self,
        Parameters(args): Parameters<KillArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::screen(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                ScreenRequest { id: args.id },
            )
            .await
            .map(|r| jobs::render_screen(&r)),
        )
    }

    #[tool(
        name = "job_stats",
        description = "What a job is consuming — CPU and memory, now and at peak over the last \
                       minute. Use this to find what is loading a machine. Limits a job declares \
                       are advisory: a breach is reported here and nothing acts on it."
    )]
    async fn job_stats(
        &self,
        Parameters(args): Parameters<KillArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::stats(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                StatsRequest { id: args.id },
            )
            .await
            .map(|r| jobs::render_stats(&r)),
        )
    }

    #[tool(
        name = "job_kill",
        description = "Stop a running job. Its record and its output stay readable afterwards, \
                       so you can still see how it ended."
    )]
    async fn job_kill(
        &self,
        Parameters(args): Parameters<KillArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hub = match self.need_hub().await {
            Ok(hub) => hub,
            Err(no_hub) => return Ok(no_hub),
        };
        reply(
            jobs::kill(
                &hub,
                target(&args.node, &args.link),
                &connector_id(&args.connector),
                KillRequest {
                    id: args.id,
                    caller: jobs::session_caller(),
                },
            )
            .await
            .map(|r| format!("kill requested for job {}", r.id)),
        )
    }
}

/// The `tools/list` reply. Written out rather than left to the macro, which
/// omits the freshness fields MCP 2026-07-28 requires (see the test below).
/// The list depends on this session's environment, so it is private to it;
/// a zero ttl asks the client to re-list rather than trust a cache.
fn tool_list(tools: Vec<rmcp::model::Tool>) -> rmcp::model::ListToolsResult {
    let mut reply = rmcp::model::ListToolsResult::with_all_items(tools);
    reply.ttl_ms = Some(0);
    reply.cache_scope = Some(rmcp::model::CacheScope::Private);
    reply
}

/// The server's own identity and hints.
///
/// Named for **this** shim, not for the SDK that carries it: the agent sees this
/// name when choosing a tool, and "rmcp" would tell it nothing about what is on
/// the other side.
#[tool_handler]
impl ServerHandler for ShimServer {
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        Ok(tool_list(self.listed_tools()))
    }

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "kudo-shim".to_string();
        info.server_info.version = env!("CARGO_PKG_VERSION").to_string();
        info.instructions = Some(match self.hub.as_ref() {
            HubState::Dialled(_) | HubState::Lazy { .. } => {
                "Reaches capabilities running on machines in the kudo mesh, through the hub. \
                      Start with `nodes` to see which machines are connected and what each can \
                      do; address later work to a node by name."
                    .to_string()
            }
            HubState::Absent(why) => format!(
                "This session has no hub ({why}): the worker tools — context_read, evidence, \
                 pull, submit, release, inbox — work against the context store directly; the job \
                 and node tools are unavailable."
            ),
        });
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compile-time assertion that the macro produced a real MCP server:
    /// without this, a change to the attribute could silently leave the tools
    /// unrouted and nothing would fail until a client connected.
    /// The agent CLI negotiates MCP 2026-07-28, which requires `ttlMs` and
    /// `cacheScope` on a `tools/list` reply. Without them it rejects the reply
    /// and the session silently has no kudo tools at all.
    #[test]
    fn the_tool_list_carries_what_2026_07_28_requires() {
        let reply = serde_json::to_value(tool_list(ShimServer::tool_router().list_all())).unwrap();
        assert!(reply["ttlMs"].is_u64(), "{reply}");
        assert!(
            matches!(reply["cacheScope"].as_str(), Some("public" | "private")),
            "{reply}"
        );
        assert!(reply["tools"].as_array().is_some_and(|t| !t.is_empty()));
    }

    /// A node's recipes are a tool, and `job_start` takes `node_recipe`.
    // implements: 7735b6fcb04483f5bb958b825bd6e7057146db208d5f5e5b9da25d4ad9dcff01
    #[test]
    fn recipes_are_a_tool_and_job_start_can_mean_one() {
        let tools = ShimServer::tool_router().list_all();
        assert!(tools.iter().any(|t| t.name == "recipes"));
        let start = tools.iter().find(|t| t.name == "job_start").unwrap();
        let schema = serde_json::to_value(&*start.input_schema).unwrap();
        assert!(schema["properties"]["node_recipe"].is_object(), "{schema}");
    }

    /// A worker session sees the worker tools beside the job tools.
    #[test]
    fn the_worker_tools_are_routed() {
        let names: Vec<String> = ShimServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        for tool in [
            "context_read",
            "pull",
            "submit",
            "release",
            "inbox",
            "evidence",
        ] {
            assert!(
                names.iter().any(|n| n == tool),
                "{tool} missing from {names:?}"
            );
        }
    }

    /// The submit tool takes the typed submission, not one free-text log.
    // implements: 4e67251bc978f34e7d6ce4278eb1503f9e803e51f0057d4d5954c701830f22b9@1b7ca6a98f08e1312c89a21ba22a046c2f9677965e58dd2fde0cae2756307a74
    #[test]
    fn the_submit_tool_takes_the_typed_submission() {
        let submit = ShimServer::tool_router()
            .list_all()
            .into_iter()
            .find(|t| t.name == "submit")
            .unwrap();
        let schema = serde_json::to_value(&submit.input_schema).unwrap();
        for field in [
            "claim",
            "head",
            "log",
            "established",
            "claimed",
            "ruled_out",
            "gaps",
            "base",
        ] {
            assert!(
                schema["properties"].get(field).is_some(),
                "{field} missing: {schema}"
            );
        }
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, ["claim", "head", "log"], "{schema}");
    }

    #[test]
    fn the_macro_generates_a_server_handler() {
        fn assert_handler<T: rmcp::ServerHandler>() {}
        assert_handler::<ShimServer>();
    }
}
