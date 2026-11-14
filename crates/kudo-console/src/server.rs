//! The console server's state: one context store (with this machine's
//! mirror), and the hub — for nodes, and for the loop's events, which bump a
//! version the page waits on.
//!
//! implements: 03045b42c8f5eb9f6b68ffe4b372e8560b43d4da21c4a77c6ddab998713b7c6d@03045b42c8f5eb9f6b68ffe4b372e8560b43d4da21c4a77c6ddab998713b7c6d

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dioxus::prelude::ServerFnError;
use kudo_context::ContextStore;

use crate::model::{ConsoleData, JobOutput, NodeJobs, NodeView, PaneRef, TerminalView};

/// Bumped by every event the hub carries under `kudo/`, and by the console's
/// own writes.
static VERSION: AtomicU64 = AtomicU64::new(1);
static MOVED: OnceLock<tokio::sync::Notify> = OnceLock::new();
static FOLLOWING: OnceLock<()> = OnceLock::new();
/// One store for the server's life, so its mirror spacing and offline backoff
/// hold across requests.
static STORE: Mutex<Option<Arc<ContextStore>>> = Mutex::new(None);
static HUB: OnceLock<Arc<kudo_shim::HubClient>> = OnceLock::new();

fn moved() -> &'static tokio::sync::Notify {
    MOVED.get_or_init(tokio::sync::Notify::new)
}

fn bump() {
    VERSION.fetch_add(1, Ordering::Relaxed);
    moved().notify_waiters();
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Refuse what another web page could make a browser send.
pub(crate) async fn guarded() -> Result<(), ServerFnError> {
    use dioxus::fullstack::{FullstackContext, HeaderMap, Method};
    let headers = FullstackContext::extract::<HeaderMap, _>().await?;
    let method = FullstackContext::extract::<Method, _>().await?;
    crate::guard::allowed(&method, &headers).map_err(ServerFnError::new)
}

fn store() -> Result<Arc<ContextStore>, String> {
    let mut held = STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(store) = held.as_ref() {
        return Ok(store.clone());
    }
    let dir = kudo_shim::cli::default_store_dir()
        .ok_or_else(|| "cannot find the context store: set KUDO_CONTEXT_STORE".to_owned())?;
    let store = Arc::new(kudo_shim::cli::open_store(
        &dir,
        kudo_shim::cli::mirror_from_env().as_deref(),
        true,
    )?);
    *held = Some(store.clone());
    Ok(store)
}

pub(crate) async fn console_data(selected: Option<String>) -> ConsoleData {
    follow_events();
    // The store's reads (and its mirror's syncs) block: off the runtime.
    let read = tokio::task::spawn_blocking(move || {
        store().and_then(|store| {
            let at = now();
            let mut data = crate::views::console(&store, selected, at);
            let pending = store.pending_mcp(at).map_err(|e| e.to_string())?;
            data.mcp_approvals = crate::views::mcp_approvals(pending, at);
            Ok(data)
        })
    })
    .await;
    let mut data = match read {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => ConsoleData {
            error: Some(e),
            ..ConsoleData::default()
        },
        Err(e) => ConsoleData {
            error: Some(e.to_string()),
            ..ConsoleData::default()
        },
    };
    data.nodes = nodes().await;
    (data.approvals, data.approvals_unknown) = approvals(&data.nodes).await;
    data
}

/// Every node's jobs waiting for a verdict, named by the context that asked,
/// asked of all nodes at once; and the nodes whose queue could not be read.
// implements: kudo:daf63591a53811da5e761c847a528776758b90ee07fc55f8c16ce14789be1f44
async fn approvals(nodes: &[NodeView]) -> (Vec<crate::model::ApprovalView>, Vec<String>) {
    if nodes.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Ok(hub) = hub().await else {
        return (Vec::new(), nodes.iter().map(|n| n.id.clone()).collect());
    };
    let asks = nodes.iter().map(|node| {
        let hub = &hub;
        async move {
            let ask = kudo_shim::jobs::pending(
                hub,
                kudo_shim::route::Target::node(node.id.clone()),
                kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
            );
            let got = tokio::time::timeout(Duration::from_secs(2), ask).await;
            (node.id.clone(), got)
        }
    });
    let mut pending = Vec::new();
    let mut unknown = Vec::new();
    for (node, got) in futures_util::future::join_all(asks).await {
        match got {
            Ok(Ok(reply)) => pending.extend(reply.jobs.into_iter().map(|j| (node.clone(), j))),
            _ => unknown.push(node),
        }
    }
    if pending.is_empty() {
        return (Vec::new(), unknown);
    }
    let asked = tokio::task::spawn_blocking(|| {
        store()
            .ok()
            .and_then(|s| crate::views::job_contexts(&s).ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    (crate::views::approvals(pending, &asked), unknown)
}

/// The human's verdict on a waiting job, sent as the console; what it led to.
pub(crate) async fn decide(
    node: String,
    job: String,
    verdict: String,
    edited: Option<Vec<String>>,
) -> Result<String, String> {
    let wire = crate::views::verdict_from(&verdict)
        .ok_or_else(|| format!("{verdict:?} is not a verdict"))?;
    // An edit is a whole argv: an empty one is refused, never read as "no
    // edit" (which would approve the original command).
    if edited.as_ref().is_some_and(Vec::is_empty) {
        return Err("an edited command cannot be empty".into());
    }
    let hub = hub().await?;
    let reply = kudo_shim::jobs::decide(
        &hub,
        kudo_shim::route::Target::node(node),
        kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        moco_job::wire::DecideRequest {
            id: job.clone(),
            verdict: wire,
            edited_argv: edited.unwrap_or_default(),
            caller: moco_job::wire::WireCaller::Console,
        },
    )
    .await;
    // Either way the queue moved (or was stale): read it again.
    bump();
    let reply = reply.map_err(|e| e.to_string())?;
    Ok(format!(
        "job {job}: {} under {}",
        kudo_shim::jobs::status_word(&reply.status),
        kudo_shim::jobs::verdict_word(reply.verdict)
    ))
}

/// The author's single-use verdict on one parked MCP call.
pub(crate) async fn decide_mcp(request: String, verdict: String) -> Result<String, String> {
    let verdict = match verdict.as_str() {
        "allow-once" => kudo_context::McpVerdict::Allow,
        "deny-once" => kudo_context::McpVerdict::Deny,
        _ => return Err(format!("{verdict:?} is not a single-use MCP verdict")),
    };
    let decided = tokio::task::spawn_blocking(move || {
        let store = store()?;
        let at = now();
        let request = store
            .pending_mcp(at)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|pending| pending.id == request)
            .ok_or_else(|| "the MCP request is no longer waiting".to_owned())?;
        store
            .decide_mcp(&request.context, &request.id, verdict, at)
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(format!("MCP request {}: {verdict:?}", request.id))
    })
    .await
    .map_err(|e| e.to_string())??;
    bump();
    Ok(decided)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// A node's governed jobs, in words; a node that cannot be read says why.
// implements: 2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655@2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655
pub(crate) async fn node_jobs(node: String) -> NodeJobs {
    let read = async {
        let hub = hub().await?;
        let ask = kudo_shim::jobs::list(
            &hub,
            kudo_shim::route::Target::node(node.clone()),
            kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        );
        match tokio::time::timeout(Duration::from_secs(3), ask).await {
            Ok(Ok(reply)) => Ok(reply.jobs),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!("{node} did not answer")),
        }
    };
    match read.await {
        Ok(jobs) => NodeJobs {
            jobs: crate::views::job_views(jobs, now_ms()),
            node,
            error: None,
        },
        Err(e) => NodeJobs {
            node,
            jobs: Vec::new(),
            error: Some(e),
        },
    }
}

/// A job's output from `offset`.
pub(crate) async fn job_output(
    node: String,
    job: String,
    offset: u64,
) -> Result<JobOutput, String> {
    let hub = hub().await?;
    let reply = kudo_shim::jobs::tail(
        &hub,
        kudo_shim::route::Target::node(node.clone()),
        kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        moco_job::wire::TailRequest {
            id: job.clone(),
            offset,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(crate::views::job_output(&node, &job, &reply))
}

/// Kill a job, as the console; what it led to. Only a job the node lists as
/// still waiting or running: an id from a stale page kills nothing.
pub(crate) async fn kill_job(node: String, job: String) -> Result<String, String> {
    let listed = node_jobs(node.clone()).await;
    if let Some(e) = listed.error {
        return Err(e);
    }
    if !listed.jobs.iter().any(|j| j.id == job && j.live) {
        return Err(format!("job {job} on {node} is not running"));
    }
    let hub = hub().await?;
    let reply = kudo_shim::jobs::kill(
        &hub,
        kudo_shim::route::Target::node(node.clone()),
        kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        moco_job::wire::KillRequest {
            id: job,
            caller: moco_job::wire::WireCaller::Console,
        },
    )
    .await;
    bump();
    let reply = reply.map_err(|e| e.to_string())?;
    Ok(format!("job {} on {node}: asked to stop", reply.id))
}

/// A recorded pane's screen.
pub(crate) async fn pane_screen(pane: PaneRef) -> Result<TerminalView, String> {
    let screen = screen(&pane).await?;
    Ok(TerminalView { pane, screen })
}

/// Whether the console recorded this pane: typing reaches only panes it
/// started, never any job on any node.
async fn recorded(pane: &PaneRef) -> Result<bool, String> {
    let pane = pane.clone();
    tokio::task::spawn_blocking(move || {
        let store = store()?;
        for card in store.contexts().map_err(|e| e.to_string())? {
            let panes = store.panes(&card.id).map_err(|e| e.to_string())?;
            if panes
                .iter()
                .any(|p| p.node == pane.node && p.job == pane.job)
            {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The process's hub, dialled within two seconds and reused until it redials itself.
async fn hub() -> Result<Arc<kudo_shim::HubClient>, String> {
    let addr = kudo_shim::hub_addr_from_env().ok_or("no hub: set KUDO_HUB_ADDR")?;
    if let Some(hub) = HUB.get() {
        return Ok(hub.clone());
    }
    match tokio::time::timeout(Duration::from_secs(2), kudo_shim::HubClient::dial(&addr)).await {
        Ok(Ok(hub)) => {
            let hub = Arc::new(hub);
            if HUB.set(hub.clone()).is_ok() {
                Ok(hub)
            } else {
                HUB.get()
                    .cloned()
                    .ok_or_else(|| "the hub connection was not retained".to_owned())
            }
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("the hub at {addr} did not answer")),
    }
}

/// A pane's screen, as its node renders it.
async fn screen(pane: &PaneRef) -> Result<String, String> {
    let hub = hub().await?;
    let reply = kudo_shim::jobs::screen(
        &hub,
        kudo_shim::Target::node(pane.node.clone()),
        kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        moco_job::wire::ScreenRequest {
            id: pane.job.clone(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&reply.bytes).into_owned())
}

/// Start a worker in a pane on `node`; the pane. The launch policy is the
/// shim's — the console only calls it.
pub(crate) async fn start_worker(context: String, node: String) -> Result<PaneRef, String> {
    let store = tokio::task::spawn_blocking(store)
        .await
        .map_err(|e| e.to_string())??;
    let hub = hub().await?;
    let pane = kudo_shim::panes::start(&hub, store, &context, &node, now()).await?;
    bump();
    Ok(PaneRef {
        node: pane.node,
        job: pane.job,
    })
}

/// Type into a pane, as the console.
pub(crate) async fn type_into(node: String, job: String, bytes: Vec<u8>) -> Result<(), String> {
    let pane = PaneRef { node, job };
    if !recorded(&pane).await? {
        return Err("that is not a pane this console started".to_owned());
    }
    let PaneRef { node, job } = pane;
    let hub = hub().await?;
    kudo_shim::jobs::input(
        &hub,
        kudo_shim::Target::node(node),
        kudo_shim::jobs::DEFAULT_JOB_CONNECTOR,
        moco_job::wire::InputRequest {
            id: job,
            bytes,
            caller: moco_job::wire::WireCaller::Console,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

/// The nodes the hub reaches; none when no hub is configured, it is down, or
/// it does not answer within two seconds.
async fn nodes() -> Vec<NodeView> {
    let ask = async {
        let hub = hub().await.ok()?;
        hub.topology().await.ok()
    };
    match tokio::time::timeout(Duration::from_secs(2), ask).await {
        Ok(Some(snapshot)) => {
            let named = kudo_shim::panes::worker_nodes_from(|k| std::env::var(k).ok());
            snapshot
                .nodes
                .into_iter()
                .map(|n| NodeView {
                    may_start: kudo_shim::panes::may_start(&n.node.0, named.as_deref()),
                    id: n.node.0,
                    links: n.links.into_iter().map(|l| l.link.0).collect(),
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) async fn enqueue(context: String, ask: String) -> Result<String, String> {
    let said = tokio::task::spawn_blocking(move || {
        let dir = kudo_shim::cli::default_store_dir()
            .ok_or_else(|| "cannot find the context store: set KUDO_CONTEXT_STORE".to_owned())?;
        kudo_shim::cli::context_with(
            &dir,
            kudo_shim::cli::mirror_from_env().as_deref(),
            kudo_shim::hub_addr_from_env().as_deref(),
            &["enqueue".to_owned(), context, ask],
            now(),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    bump();
    Ok(said)
}

pub(crate) async fn wait_version(after: u64) -> u64 {
    follow_events();
    let now = VERSION.load(Ordering::Relaxed);
    if now != after {
        return now;
    }
    let _ = tokio::time::timeout(Duration::from_secs(25), moved().notified()).await;
    VERSION.load(Ordering::Relaxed)
}

/// Subscribe once to the hub's `kudo/` events; each one bumps the version.
/// When an established subscription ends (hub restart, dropped for falling
/// behind) the version is bumped once — whatever was missed is re-read — and
/// it subscribes again. A hub that cannot subscribe at all (absent, too old
/// for events) is retried with backoff and bumps nothing: no hidden timer.
fn follow_events() {
    if FOLLOWING.set(()).is_err() {
        return;
    }
    let Some(addr) = kudo_shim::hub_addr_from_env() else {
        return;
    };
    tokio::spawn(async move {
        let mut wait = Duration::from_secs(2);
        loop {
            if follow_once(&addr).await {
                bump();
                wait = Duration::from_secs(2);
            } else {
                wait = (wait * 2).min(Duration::from_secs(60));
            }
            tokio::time::sleep(wait).await;
        }
    });
}

/// Follow one subscription until it ends; whether it was ever live.
async fn follow_once(addr: &str) -> bool {
    use hub_protocol::HubEvent;
    use hub_protocol::service::ConsumerApiClient;
    let Ok(socket) = tokio::net::TcpStream::connect(addr).await else {
        return false;
    };
    let Ok(connection) = vox::initiator_on(vox::transport::tcp::StreamLink::tcp(socket))
        .establish_connection()
        .await
    else {
        return false;
    };
    let Ok(consumer): Result<ConsumerApiClient, _> = connection.open_lane().await else {
        return false;
    };
    let (tx, mut rx) = vox::channel::<HubEvent>();
    let subscription = tokio::spawn(async move {
        let _ = consumer.subscribe_events("kudo/".to_owned(), tx).await;
    });
    // The hub's first event says the subscription is live.
    let live = matches!(
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await,
        Ok(Ok(Some(_)))
    );
    if live {
        while let Ok(Some(_event)) = rx.recv().await {
            bump();
        }
    }
    subscription.abort();
    live
}
