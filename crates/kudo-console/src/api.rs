//! The console's server functions: thin calls into the context engine and the
//! hub — nothing a second face would need lives here.
//!
//! implements: d7562b1ac2f15d346f0a98561dc3da3351fabcd807f4ce4bbcd614f2f2a73c1f@9ac5369cd207a2da44b4ac1698c0b6725ef7ab4d94798e3411253a0c60d32c29

use dioxus::prelude::*;

use crate::model::{ConsoleData, JobOutput, NodeJobs, PaneRef, TerminalView};

/// Everything the panes draw from, with `selected` (an id or unique prefix)
/// as the selected context.
#[server]
pub async fn console_data(selected: Option<String>) -> Result<ConsoleData, ServerFnError> {
    crate::server::guarded().await?;
    Ok(crate::server::console_data(selected).await)
}

/// A pane's screen, for the terminal pane's poll.
#[server]
pub async fn pane_screen(pane: PaneRef) -> Result<TerminalView, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::pane_screen(pane)
        .await
        .map_err(ServerFnError::new)
}

/// Enqueue `ask` under the context `context` (an id or unique prefix).
#[server]
pub async fn enqueue(context: String, ask: String) -> Result<String, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::enqueue(context, ask)
        .await
        .map_err(ServerFnError::new)
}

/// Wait until something has moved since `after` (the hub's events), or a
/// while has passed; the version now. The page long-polls this instead of
/// re-reading on a timer.
#[server]
pub async fn wait_version(after: u64) -> Result<u64, ServerFnError> {
    crate::server::guarded().await?;
    Ok(crate::server::wait_version(after).await)
}

/// Start a worker for `context` on `node`, in a pane the console can take
/// over: the job id.
#[server]
pub async fn start_worker(context: String, node: String) -> Result<PaneRef, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::start_worker(context, node)
        .await
        .map_err(ServerFnError::new)
}

/// Type `bytes` into the pane `job` on `node` — the human's take over.
#[server]
pub async fn type_into(node: String, job: String, bytes: Vec<u8>) -> Result<(), ServerFnError> {
    crate::server::guarded().await?;
    crate::server::type_into(node, job, bytes)
        .await
        .map_err(ServerFnError::new)
}

/// Answer the gate for the job `job` on `node`: `verdict` is `allow-once`,
/// `allow-always`, `deny-once` or `deny-always`; `edited`, when given, is the
/// argv to run instead (allow verdicts only; never empty). What it led to.
#[server]
pub async fn decide(
    node: String,
    job: String,
    verdict: String,
    edited: Option<Vec<String>>,
) -> Result<String, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::decide(node, job, verdict, edited)
        .await
        .map_err(ServerFnError::new)
}

/// Allow once or deny once for one parked external MCP call.
#[server]
pub async fn decide_mcp(request: String, verdict: String) -> Result<String, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::decide_mcp(request, verdict)
        .await
        .map_err(ServerFnError::new)
}

/// A node's governed jobs, for the node view's poll.
#[server]
pub async fn node_jobs(node: String) -> Result<NodeJobs, ServerFnError> {
    crate::server::guarded().await?;
    Ok(crate::server::node_jobs(node).await)
}

/// A job's output from `offset`, with its state, for the job view's poll.
#[server]
pub async fn job_output(
    node: String,
    job: String,
    offset: u64,
) -> Result<JobOutput, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::job_output(node, job, offset)
        .await
        .map_err(ServerFnError::new)
}

/// Kill the job `job` on `node`, as the console; what it led to.
#[server]
pub async fn kill_job(node: String, job: String) -> Result<String, ServerFnError> {
    crate::server::guarded().await?;
    crate::server::kill_job(node, job)
        .await
        .map_err(ServerFnError::new)
}
