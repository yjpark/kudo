//! The agent shim binary: an MCP server on stdio.
//!
//! Launched as a child of the agent session, so its **working directory is the
//! session's project directory** — which is what makes workspace identity free
//! and unconfigurable-by-mistake. Nothing here computes what that directory
//! *means*; turning a cwd into a workspace is the engine's rule, and a second
//! face would need the same one.
//!
//! implements: 121ac6ebe48b717b93e775f5a0526076a9230ec0e10e748dbcbaf181bf758120

use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

use kudo_worker::WorkerSetup;

use kudo_shim::startup::{Startup, plan_startup};
use kudo_shim::{HubClient, ShimServer, call_timeout_from_env, hub_addr_from_env};

/// What `kudo-shim --help` prints.
const USAGE: &str = "\
kudo-shim — kudo's agent-side binary

usage:
  kudo-shim                      an MCP server on stdio (the CLI starts it): job
                                 tools over the hub, and worker tools in a worker
  kudo-shim hook <event>         answer a worker's CLI hook (session-start, stop,
                                 pre-compact, user-prompt-submit)
  kudo-shim context <command>    drive a context by hand (new, enqueue, show,
                                 list, trace, why, repo add, check, wake-node)
  kudo-shim worker <context> [--repo <name>] [--headless]
                                 start a worker session here
  kudo-shim kimi-input           bridge console input to Kimi's terminal
  kudo-shim waker                wake a worker when a context's check comes due
                                 (a user service on one node)
  kudo-shim --version | --help
";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `kudo-shim hook <event>`: the CLI's hooks. Answered from the context
    // store before anything dials the hub — a worker's loop must survive the
    // hub and the node restarting.
    let args: Vec<String> = std::env::args().collect();
    // implements: 35e0ccd93acf733faaacad05140d9932384b173b1a02353b7145c879441b2c6a@35e0ccd93acf733faaacad05140d9932384b173b1a02353b7145c879441b2c6a
    match args.get(1).map(String::as_str) {
        Some("--version" | "-V" | "version") => {
            println!("kudo-shim {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            return Ok(());
        }
        _ => {}
    }
    if args.get(1).map(String::as_str) == Some("hook") {
        std::process::exit(run_hook(
            args.get(2).map(String::as_str).unwrap_or_default(),
        ));
    }

    if args.get(1).map(String::as_str) == Some("kimi-input") {
        run_kimi_input()?;
        return Ok(());
    }

    // The waker: a long-running loop on one node.
    if args.get(1).map(String::as_str) == Some("waker") {
        std::process::exit(run_waker().await);
    }

    // The local commands: drive a context by hand, start a worker here.
    if let Some(cmd @ ("context" | "worker")) = args.get(1).map(String::as_str) {
        let Some(store) = kudo_shim::cli::default_store_dir() else {
            eprintln!("cannot find a place for the context store: set KUDO_CONTEXT_STORE");
            std::process::exit(2);
        };
        if cmd == "worker" {
            let (ctx, repo, mode) = match &args[2..] {
                [ctx] => (ctx, None, kudo_worker::harness::LaunchMode::Interactive),
                [ctx, flag] if flag == "--headless" => {
                    (ctx, None, kudo_worker::harness::LaunchMode::Headless)
                }
                [ctx, flag, repo] if flag == "--repo" => (
                    ctx,
                    Some(repo.as_str()),
                    kudo_worker::harness::LaunchMode::Interactive,
                ),
                [ctx, flag, repo, headless] if flag == "--repo" && headless == "--headless" => (
                    ctx,
                    Some(repo.as_str()),
                    kudo_worker::harness::LaunchMode::Headless,
                ),
                [ctx, headless, flag, repo] if headless == "--headless" && flag == "--repo" => (
                    ctx,
                    Some(repo.as_str()),
                    kudo_worker::harness::LaunchMode::Headless,
                ),
                _ => {
                    eprintln!("usage: kudo-shim worker <context> [--repo <name>] [--headless]");
                    std::process::exit(2);
                }
            };
            eprintln!("{}", kudo_shim::cli::exec_worker(&store, ctx, repo, mode));
            std::process::exit(1);
        }
        match kudo_shim::cli::context(&store, &args[2..], unix_now()) {
            Ok(text) => print!("{text}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    // Diagnostics go to stderr: stdout *is* the MCP transport, so a stray line
    // there corrupts the protocol rather than informing anyone.
    //
    // A worker session needs the context store, not the hub: it starts without
    // one (its job tools say why). A session that is only job tools needs the
    // hub, and refuses to start rather than look healthy while failing every
    // call.
    let worker = WorkerSetup::from_env();
    let server = match plan_startup(hub_addr_from_env(), worker.is_some()) {
        // A worker dials the hub when a job tool first needs it, so a hub that
        // is down or slow never holds up its loop.
        Startup::Dial(addr) if worker.is_some() => ShimServer::lazy(addr, call_timeout_from_env()),
        Startup::Dial(addr) => {
            match tokio::time::timeout(Duration::from_secs(3), HubClient::dial(&addr)).await {
                Ok(Ok(hub)) => ShimServer::new(Arc::new(match call_timeout_from_env() {
                    Some(timeout) => hub.with_timeout(timeout),
                    None => hub,
                })),
                Ok(Err(e)) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                Err(_) => {
                    eprintln!("the hub at {addr} did not answer within 3s");
                    std::process::exit(1);
                }
            }
        }
        Startup::WithoutHub(why) => {
            eprintln!("kudo-shim: {why}; serving the worker tools without a hub");
            ShimServer::without_hub(why)
        }
        Startup::Refuse(why) => {
            eprintln!("{why}");
            std::process::exit(2);
        }
    };

    let (stdin, stdout) = stdio();
    let service = server.with_worker(worker).serve((stdin, stdout)).await?;
    service.waiting().await?;
    Ok(())
}

fn run_kimi_input() -> std::io::Result<()> {
    use std::io::{Read, Write};
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut bytes = [0; 4096];
    loop {
        let read = input.read(&mut bytes)?;
        if read == 0 {
            return output.flush();
        }
        for byte in &bytes[..read] {
            if matches!(*byte, b'\r' | b'\n') {
                output.write_all(b"\x1b[13u")?;
            } else {
                output.write_all(&[*byte])?;
            }
        }
        output.flush()?;
    }
}

/// Run one hook: the event JSON on stdin, the decision on stdout.
fn run_hook(event: &str) -> i32 {
    use std::io::{Read, Write};

    let Some(setup) = WorkerSetup::from_env() else {
        // Not a worker session: a hook installed here has nothing to decide.
        return 0;
    };
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("kudo-shim hook: reading the event: {e}");
        return 1;
    }
    let reply = kudo_shim::hook::run(event, &stdin, &setup, unix_now());
    if !reply.stdout.is_empty() {
        let mut out = std::io::stdout();
        let _ = out
            .write_all(reply.stdout.as_bytes())
            .and_then(|()| out.flush());
    }
    reply.code
}

/// Seconds since the Unix epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `kudo-shim waker`: once a minute, wake a worker for each context whose
/// check came due and that runs none. Runs until stopped.
// implements: 43643bef4da543e855bafcb2308401df3c7b2f1c5a4ef03ac69c06b7a4bd2d2a@f76f4215e264261e284792a84ffb31b5fbc7f3498b04b00dccf45aa79cac60a2
async fn run_waker() -> i32 {
    let Some(dir) = kudo_shim::cli::default_store_dir() else {
        eprintln!("cannot find the context store: set KUDO_CONTEXT_STORE");
        return 2;
    };
    let Some(addr) = hub_addr_from_env() else {
        eprintln!("the waker starts workers through the hub: set KUDO_HUB_ADDR");
        return 2;
    };
    let store = match kudo_shim::cli::open_store(
        &dir,
        kudo_shim::cli::mirror_from_env().as_deref(),
        true,
    ) {
        Ok(store) => Arc::new(store),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // The nodes it may start on are the console's, and must be said: unset,
    // the launch policy would allow any node.
    let Some(nodes) = kudo_shim::panes::worker_nodes_from(|k| std::env::var(k).ok()) else {
        eprintln!(
            "the waker starts workers only on the nodes named in {}: set it",
            kudo_shim::panes::WORKER_NODES_ENV
        );
        return 2;
    };
    let own = kudo_shim::waker::default_node(&kudo_shim::cli::node_name(), &nodes);
    let mut backoff = kudo_shim::waker::Backoff::default();
    let quiet = kudo_worker::env::DEFAULT_CLAIM_TTL;
    println!(
        "kudo waker: looking every {}s; workers start on {}; woken workers default to {own}",
        kudo_shim::waker::EVERY.as_secs(),
        nodes.join(", ")
    );
    let mut hub = None;
    loop {
        if hub.is_none() {
            match tokio::time::timeout(Duration::from_secs(5), HubClient::dial(&addr)).await {
                Ok(Ok(dialled)) => hub = Some(dialled),
                Ok(Err(e)) => eprintln!("kudo waker: the hub at {addr}: {e}"),
                Err(_) => eprintln!("kudo waker: the hub at {addr} did not answer"),
            }
        }
        if let Some(connected) = hub.as_ref() {
            for line in kudo_shim::waker::wake_once(
                connected,
                store.clone(),
                unix_now(),
                &own,
                quiet,
                &mut backoff,
            )
            .await
            {
                println!("{line}");
            }
        }
        tokio::time::sleep(kudo_shim::waker::EVERY).await;
    }
}
