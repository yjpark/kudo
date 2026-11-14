//! The shim's local commands: create and drive a context, and start a worker
//! in this terminal. A thin face over the context engine — every answer is the
//! engine's.

use std::path::{Path, PathBuf};

use kudo_context::{ContextId, ContextStore, Repo, TraceEvent};

const USAGE: &str = "usage: kudo-shim context new <title>\n       \
                     kudo-shim context enqueue <context> [--due <when>] [--after <item>]… <ask>\n       \
                     kudo-shim context check <context> <what> --pass <condition> [--window <window>] [--due <when>]\n       \
                     kudo-shim context wake-node <context> <node>\n       \
                     kudo-shim context show <context>\n       \
                     kudo-shim context list\n       \
                     kudo-shim context trace <context>\n       \
                     kudo-shim context why <context> [head-version]\n       \
                     kudo-shim context repo add <context> <name> [path] [--node <node>]\n       \
                     kudo-shim worker <context> [--repo <name>] [--headless]";

/// Where the context store lives: `KUDO_CONTEXT_STORE`, else
/// `$XDG_STATE_HOME/kudo/contexts`, else `~/.local/state/kudo/contexts`.
pub fn default_store_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(kudo_worker::env::STORE_ENV) {
        return Some(PathBuf::from(dir));
    }
    let state = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| Path::new(&h).join(".local/state"))
        })?;
    Some(state.join("kudo/contexts"))
}

/// A due time as typed: `+<n>m`, `+<n>h`, `+<n>d` from now, or an RFC 3339
/// time.
// implements: 716432a845eb22babf957938aaa9a9c0f3cd3bea3ebee24c89a5147a795e7e2b@80a3c962223cc4b37ffceccfdfb83cb9d143fd3e956d4cbe19c7ec3e93774ffd
pub fn parse_when(when: &str, now: i64) -> Result<i64, String> {
    let bad = || format!("{when:?} is not a time: write +30m, +2h, +1d, or an RFC 3339 time");
    if let Some(rel) = when.strip_prefix('+') {
        let Some((cut, _)) = rel.char_indices().last() else {
            return Err(bad());
        };
        let (n, unit) = rel.split_at(cut);
        let n: i64 = n.parse().map_err(|_| bad())?;
        if n <= 0 {
            return Err(bad());
        }
        let secs = match unit {
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            _ => return Err(bad()),
        };
        return n
            .checked_mul(secs)
            .and_then(|d| now.checked_add(d))
            .ok_or_else(bad);
    }
    when.parse::<jiff::Timestamp>()
        .map(|t| t.as_second())
        .map_err(|_| bad())
}

/// Run `kudo-shim context <args…>` against the store in `store_dir`; the text
/// to print, or an error.
pub fn context(store_dir: &Path, args: &[String], now: i64) -> Result<String, String> {
    context_with(
        store_dir,
        mirror_from_env().as_deref(),
        crate::hub_addr_from_env().as_deref(),
        args,
        now,
    )
}

/// [`context`], with the mirror and the hub (for its events) given rather
/// than read from the environment.
pub fn context_with(
    store_dir: &Path,
    mirror: Option<&str>,
    hub: Option<&str>,
    args: &[String],
    now: i64,
) -> Result<String, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["new", title @ ..] if !title.is_empty() => {
            let title = title.join(" ");
            let store = open_or_init_with(store_dir, mirror)?;
            let head = format!(
                "# {title}\nstate:       active — new\nwhere:       nothing done yet\n\
                 next_action: pull the first item · who: worker\n"
            );
            let id = store
                .create_context(&title, &head, now)
                .map_err(|e| e.to_string())?;
            Ok(format!("{}\n", id.0))
        }
        ["check", ctx, what, flags @ ..] => {
            let mut check = kudo_context::Check {
                what: (*what).to_owned(),
                ..Default::default()
            };
            let mut due = None;
            let mut rest = flags;
            while let [flag, value, tail @ ..] = rest {
                match *flag {
                    "--pass" => check.pass = (*value).to_owned(),
                    "--window" => check.window = (*value).to_owned(),
                    "--due" => due = Some(parse_when(value, now)?),
                    _ => return Err(USAGE.to_owned()),
                }
                rest = tail;
            }
            if !rest.is_empty() {
                return Err(USAGE.to_owned());
            }
            if check.window.is_empty() {
                check.window = "now".to_owned();
            }
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let item = store
                .enqueue_check(&ctx, &check, due, now)
                .map_err(|e| e.to_string())?;
            Ok(format!("{}\n", item.0))
        }
        ["wake-node", ctx, node] => {
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            store
                .declare_wake_node(&ctx, node, now)
                .map_err(|e| e.to_string())?;
            Ok(format!("{} wakes on {node}\n", ctx.0))
        }
        ["enqueue", ctx, rest @ ..] => {
            if let Some(flag) = rest
                .last()
                .filter(|word| matches!(**word, "--due" | "--after"))
            {
                return Err(format!("{flag} needs a value"));
            }
            let mut due = None;
            let mut after = Vec::new();
            let mut rest = rest;
            while let [flag, value, tail @ ..] = rest {
                match *flag {
                    "--due" => due = Some(parse_when(value, now)?),
                    "--after" => after.push(kudo_context::ItemId((*value).to_owned())),
                    _ => break,
                }
                rest = tail;
            }
            if rest.is_empty() || rest.join(" ").trim().is_empty() {
                return Err("an empty ask: say what to do".to_owned());
            }
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let item = store
                .enqueue_with_dependencies(&ctx, &rest.join(" "), due, &after, now)
                .map_err(|e| e.to_string())?;
            // No lease made it: the event goes under `enqueue`.
            if let Some(hub) = hub {
                let _ = crate::events::publish(
                    hub,
                    crate::events::topic(&ctx.0, "enqueue", "enqueued"),
                    format!("enqueued {}", item.0).into_bytes(),
                );
            }
            Ok(format!("enqueued {}\n", item.0))
        }
        ["show", ctx] => {
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let head = store.head(&ctx).map_err(|e| e.to_string())?;
            let ready = store.ready_count(&ctx, now).map_err(|e| e.to_string())?;
            let waiting = store.waiting_count(&ctx, now).map_err(|e| e.to_string())?;
            let submitted = store.submitted_count(&ctx).map_err(|e| e.to_string())?;
            let log = store.log(&ctx).map_err(|e| e.to_string())?;
            let inbox = store.inbox(&ctx).map_err(|e| e.to_string())?;
            let mut out = format!(
                "{}
-- {ready} ready, {waiting} waiting, {submitted} submitted, {} in the inbox
",
                head.body.trim_end(),
                inbox.len()
            );
            let later = store.not_yet_due(&ctx, now).map_err(|e| e.to_string())?;
            if let Some(next) = later.first() {
                let at = next
                    .due
                    .and_then(|d| jiff::Timestamp::from_second(d).ok())
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                out.push_str(&format!("{} not yet due (next at {at})\n", later.len()));
            }
            if let Some(node) = store.wake_node(&ctx).map_err(|e| e.to_string())? {
                out.push_str(&format!("wakes on {node}\n"));
            }
            for repo in store.repos(&ctx).map_err(|e| e.to_string())? {
                out.push_str(&format!(
                    "repo {} @ {}: {}\n",
                    repo.name,
                    repo.node,
                    repo.path.display()
                ));
            }
            for (n, entry) in log.iter().enumerate() {
                out.push_str(&format!("\n[{}] {}\n", n + 1, entry.trim_end()));
            }
            for entry in &inbox {
                out.push_str(&format!("\ninbox: {} ({})\n", entry.line, entry.lease));
            }
            Ok(out)
        }
        ["repo", "add", ctx, name, rest @ ..] => {
            let (node, path) = match rest {
                [] => (node_name(), None),
                [path] => (node_name(), Some(*path)),
                ["--node", node] => ((*node).to_owned(), None),
                [path, "--node", node] => ((*node).to_owned(), Some(*path)),
                _ => return Err(USAGE.to_owned()),
            };
            let path = match path {
                Some(p) => PathBuf::from(p),
                None => std::env::current_dir().map_err(|e| e.to_string())?,
            };
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let repo = Repo {
                name: (*name).to_owned(),
                node,
                path,
            };
            store
                .declare_repo(&ctx, &repo, now)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "declared repo {} @ {}: {}\n",
                repo.name,
                repo.node,
                repo.path.display()
            ))
        }
        ["trace", ctx] => {
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let mut out = String::new();
            for t in store.trace(&ctx).map_err(|e| e.to_string())? {
                let short = |id: &str| id.get(..SHORT_ID).unwrap_or(id).to_owned();
                let what = match &t.event {
                    TraceEvent::LeaseStart | TraceEvent::Wait => t.event.name().to_owned(),
                    TraceEvent::Handoff(c) => format!("handoff {}", short(&c.0)),
                    TraceEvent::Pull(c) => format!("pull {}", short(&c.0)),
                    TraceEvent::Refused(c) => format!("refused {}", short(&c.0)),
                    TraceEvent::Release(c) => format!("release {}", short(&c.0)),
                    TraceEvent::Submit { claim, submission } => {
                        format!("submit {} -> {}", short(&claim.0), short(&submission.0))
                    }
                    TraceEvent::LeaseEnd(why) => format!("lease-end {why}"),
                    TraceEvent::JobStart { .. }
                    | TraceEvent::JobEnd { .. }
                    | TraceEvent::ToolCall { .. } => t.event.to_line(),
                };
                let lease = t.lease.get(..8).unwrap_or(&t.lease);
                out.push_str(&format!("{}  {lease} #{}  {what}\n", t.at, t.seq));
            }
            if out.is_empty() {
                out.push_str("no trace yet\n");
            }
            Ok(out)
        }
        ["why", ctx, version @ ..] if version.len() <= 1 => {
            let store = open_store(store_dir, mirror, true)?;
            let ctx = named(&store, ctx)?;
            let version = match version.first() {
                Some(v) => (*v).to_owned(),
                None => store.head(&ctx).map_err(|e| e.to_string())?.version,
            };
            Ok(
                match store
                    .provenance(&ctx, &version)
                    .map_err(|e| e.to_string())?
                {
                    None => format!("head {version}: the first head, no submission wrote it\n"),
                    Some(p) => format!(
                        "head {version}\n  written by submission {}\n  for claim {} (attempt {})\n  \
                         answering item: {}\n  held by lease {}  (kudo-shim context trace shows it)\n",
                        p.submission.0,
                        p.claim.0,
                        p.attempt,
                        p.item.ask.lines().next().unwrap_or_default(),
                        p.lease
                    ),
                },
            )
        }
        ["list"] => {
            if mirror.is_none() && !store_dir.join(".kinora").is_dir() {
                return Ok(NO_CONTEXTS.to_owned());
            }
            let store = open_store(store_dir, mirror, true)?;
            let all = store.contexts().map_err(|e| e.to_string())?;
            if all.is_empty() {
                return Ok(NO_CONTEXTS.to_owned());
            }
            let mut out = String::new();
            for c in &all {
                let ready = store.ready_count(&c.id, now).map_err(|e| e.to_string())?;
                let submitted = store.submitted_count(&c.id).map_err(|e| e.to_string())?;
                let short = c.id.0.get(..SHORT_ID).unwrap_or(&c.id.0);
                out.push_str(&format!(
                    "{short}  {}  ({ready} ready, {submitted} submitted, created {})\n",
                    c.title, c.created
                ));
            }
            Ok(out)
        }
        _ => Err(USAGE.to_owned()),
    }
}

const NO_CONTEXTS: &str = "no contexts yet: kudo-shim context new <title>\n";

/// How much of a context id `list` shows; any unique prefix is accepted back.
const SHORT_ID: usize = 12;

/// The context a full id or a unique prefix names.
fn named(store: &ContextStore, id_or_prefix: &str) -> Result<ContextId, String> {
    store
        .resolve_context(id_or_prefix)
        .map_err(|e| e.to_string())
}

/// Open the store in `dir`, mirrored to `mirror` (a URL) when given. With a
/// mirror, a store this machine has never held is created and filled from it.
// implements: fc473f56b1b8bdbae4a31e54e7281c4aeed3c08bd03804b256ca37e1937fe66f@61632ce8ff602a5a85acd48cc9208e1ca388d7a696ce46d8459e27b85426538e
pub fn open_store(dir: &Path, mirror: Option<&str>, create: bool) -> Result<ContextStore, String> {
    if !create && !dir.join(".kinora").is_dir() {
        return Err(format!("no context store at {}", dir.display()));
    }
    let Some(url) = mirror else {
        return ContextStore::open(dir).map_err(|e| e.to_string());
    };
    let mirror = kudo_context::Mirror::from_url(url).map_err(|e| e.to_string())?;
    Ok(init_if_missing(dir)?.with_mirror(mirror))
}

/// The mirror this machine is configured with: `KUDO_CONTEXT_MIRROR`.
pub fn mirror_from_env() -> Option<String> {
    std::env::var(kudo_worker::env::MIRROR_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// Open the store in `dir` with this machine's mirror, if it has one.
pub fn open_configured(dir: &Path) -> Result<ContextStore, String> {
    open_store(dir, mirror_from_env().as_deref(), true)
}

fn open(dir: &Path) -> Result<ContextStore, String> {
    open_configured(dir)
}

fn open_or_init_with(dir: &Path, mirror: Option<&str>) -> Result<ContextStore, String> {
    match mirror {
        Some(url) => open_store(dir, Some(url), true),
        None => init_if_missing(dir),
    }
}

fn init_if_missing(dir: &Path) -> Result<ContextStore, String> {
    if dir.join(".kinora").is_dir() {
        return ContextStore::open(dir).map_err(|e| e.to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    ContextStore::init(dir).map_err(|e| e.to_string())
}

/// The lease a worker runs under: the one its launcher picked, or a fresh
/// one (stub).
pub fn lease_for(given: Option<String>) -> Result<String, String> {
    match given {
        Some(lease)
            if lease.len() == 36 && lease.chars().all(|c| c.is_ascii_hexdigit() || c == '-') =>
        {
            Ok(lease)
        }
        Some(lease) => Err(format!("{lease:?} is not a lease id (a UUID)")),
        None => kudo_worker::new_lease_id().map_err(|e| format!("cannot make a lease id: {e}")),
    }
}

/// The lease a worker runs under, from the environment as `lookup` reads it:
/// the launcher's `KUDO_GIVEN_LEASE`, never an inherited `KUDO_LEASE`.
pub fn lease_from(lookup: impl Fn(&str) -> Option<String>) -> Result<String, String> {
    lease_for(lookup(kudo_worker::env::GIVEN_LEASE_ENV))
}

/// The mirror settings to hand on by name, from these variables.
pub fn mirror_settings_from(
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    // Only what is not secret: keys reach the worker by inheritance (the
    // agent CLI hands its environment to MCP servers), never on a command line
    // anyone on the machine can read.
    vars.into_iter()
        .filter(|(k, _)| {
            k == kudo_worker::env::MIRROR_ENV || k == "AWS_ENDPOINT" || k == "AWS_REGION"
        })
        .collect()
}

/// The context store's mirror settings in this environment, to hand on to
/// the worker: `KUDO_CONTEXT_MIRROR` and the `AWS_*` variables.
fn mirror_settings() -> Vec<(String, String)> {
    mirror_settings_from(std::env::vars())
}

/// This machine's node name: `KUDO_NODE` (the node sets it for its workers),
/// else the name the node itself goes by — `NODE_ID`, else the hostname — so a
/// shim and its node never disagree on which node is this one.
pub fn node_name() -> String {
    node_name_from(
        |name| std::env::var(name).ok(),
        |path| std::fs::read_to_string(path).ok(),
    )
}

/// [`node_name`] over a given environment and file system.
pub fn node_name_from(
    var: impl Fn(&str) -> Option<String>,
    read: impl Fn(&str) -> Option<String>,
) -> String {
    match var("KUDO_NODE").map(|n| n.trim().to_owned()) {
        Some(node) if !node.is_empty() => node,
        _ => kudo_worker::env::node_name_from(var, read),
    }
}

/// Where a worker under `ctx` starts: the path of the declared repo `repo` on
/// `node`, or `None` (here) when no repo is named.
pub fn worker_dir(
    store_dir: &Path,
    ctx: &str,
    repo: Option<&str>,
    node: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(name) = repo else {
        return Ok(None);
    };
    let store = open(store_dir)?;
    let ctx = named(&store, ctx)?;
    match store.repo_on(&ctx, name, node) {
        Ok(repo) if repo.path.is_dir() => Ok(Some(repo.path)),
        Ok(repo) => Err(format!(
            "repo {name} is declared on {node} at {}, but that directory does not exist here",
            repo.path.display()
        )),
        Err(e) => Err(format!(
            "{e}: declare it with `kudo-shim context repo add {} {name} <path> --node {node}`",
            ctx.0.get(..SHORT_ID).unwrap_or(&ctx.0)
        )),
    }
}

/// Replace this process with the agent CLI, started as a worker under `ctx`.
/// Returns only on failure.
pub fn exec_worker(
    store_dir: &Path,
    ctx: &str,
    repo: Option<&str>,
    mode: kudo_worker::harness::LaunchMode,
) -> String {
    // The worker may start in a repo's directory: a relative store path must
    // not move with it.
    let store_dir = match std::path::absolute(store_dir) {
        Ok(dir) => dir,
        Err(e) => return format!("cannot resolve the store path: {e}"),
    };
    let store_dir = store_dir.as_path();
    use std::os::unix::process::CommandExt;

    // Optional: without a hub the worker's MCP server serves the worker tools
    // alone.
    let hub = crate::hub_addr_from_env();
    let ctx = match open(store_dir).and_then(|s| named(&s, ctx)) {
        Ok(ctx) => ctx,
        Err(e) => return e,
    };
    let dir = match worker_dir(store_dir, &ctx.0, repo, &node_name()) {
        Ok(dir) => dir,
        Err(e) => return e,
    };
    let shim = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => return format!("cannot find this binary: {e}"),
    };
    // A launcher that picked the lease (the console, to record the pane it
    // starts) hands it in.
    let lease = match lease_from(|k| std::env::var(k).ok()) {
        Ok(lease) => lease,
        Err(e) => return e,
    };
    let setup = kudo_worker::WorkerSetup {
        store_dir: store_dir.to_path_buf(),
        context: ctx.clone(),
        lease,
        claim_ttl: env_num(kudo_worker::env::CLAIM_TTL_ENV)
            .unwrap_or(kudo_worker::DEFAULT_CLAIM_TTL),
        max_turns: env_num(kudo_worker::env::MAX_TURNS_ENV)
            .unwrap_or(i64::from(kudo_worker::DEFAULT_MAX_TURNS))
            .try_into()
            .unwrap_or(kudo_worker::DEFAULT_MAX_TURNS),
        idle_limit: env_num(kudo_worker::env::IDLE_LIMIT_ENV)
            .unwrap_or(kudo_worker::env::DEFAULT_IDLE_LIMIT),
    };
    let harness = match kudo_worker::harness_from(|key| std::env::var(key).ok()) {
        Ok(harness) => harness,
        Err(error) => return error,
    };
    let cli = std::env::var("KUDO_AGENT_CLI").ok();
    let bypass = kudo_worker::env::bypass_from(|k| std::env::var(k).ok());
    if bypass {
        eprintln!("kudo: this node runs workers bypassing permissions");
    }
    // Only kudo's MCP server, or the user's own as well: the node's decision.
    let strict = kudo_worker::env::strict_mcp_from(|k| std::env::var(k).ok());
    if strict {
        eprintln!("kudo: this node gives workers kudo's MCP server only");
    }
    let repo = dir
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| ".".into());
    let plan = match harness.launch(
        &setup,
        cli.as_deref(),
        &shim,
        hub.as_deref(),
        &mirror_settings(),
        kudo_worker::LaunchOptions {
            mode,
            bypass,
            strict_mcp: strict,
            repo: &repo,
        },
    ) {
        Ok(plan) => plan,
        Err(error) => return error,
    };
    if let Err(error) = install_launch_files(&plan.files, &repo) {
        return error;
    }
    if let Err(error) = install_launch_links(&plan.links) {
        return error;
    }
    if !plan.plugins.is_empty() {
        let home = plan
            .env
            .iter()
            .find(|(key, _)| key == "KIMI_CODE_HOME")
            .map(|(_, value)| PathBuf::from(value));
        let Some(home) = home else {
            return "cannot find Kimi Code's data directory: set KIMI_CODE_HOME".to_owned();
        };
        let registry = home.join("plugins/installed.json");
        if let Err(error) = install_launch_plugins(&plan.plugins, &registry) {
            return error;
        }
    }
    let Some((program, rest)) = plan.argv.split_first() else {
        return "empty launch plan".to_owned();
    };
    eprintln!("kudo: worker lease {} under context {}", setup.lease, ctx.0);
    let mut command = std::process::Command::new(program);
    if let Some(dir) = &dir {
        eprintln!("kudo: in {}", dir.display());
        command.current_dir(dir);
    }
    let err = command.args(rest).envs(plan.env).exec();
    format!("cannot start {program}: {err}")
}

/// Install launch artifacts where the agent CLI will resolve them.
pub fn install_launch_files(files: &[kudo_worker::LaunchFile], repo: &Path) -> Result<(), String> {
    for file in files {
        let path = if file.path.is_absolute() {
            file.path.clone()
        } else {
            repo.join(&file.path)
        };
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            return Err(format!("cannot create {}: {error}", parent.display()));
        }
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        output
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot protect {}: {error}", path.display()))?;
        output
            .write_all(file.contents.as_bytes())
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Link read-only launch inputs into an isolated harness home.
pub fn install_launch_links(links: &[kudo_worker::harness::LaunchLink]) -> Result<(), String> {
    for link in links {
        if std::fs::symlink_metadata(&link.target).is_ok() {
            continue;
        }
        if let Some(parent) = link.target.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            return Err(format!("cannot create {}: {error}", parent.display()));
        }
        std::os::unix::fs::symlink(&link.source, &link.target).map_err(|error| {
            format!(
                "cannot link {} to {}: {error}",
                link.target.display(),
                link.source.display()
            )
        })?;
    }
    Ok(())
}

/// Register launch plugins without replacing the user's existing registry.
pub fn install_launch_plugins(
    plugins: &[kudo_worker::harness::LaunchPlugin],
    registry: &Path,
) -> Result<(), String> {
    if let Some(parent) = registry.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Err(format!("cannot create {}: {error}", parent.display()));
    }
    let _lock = RegistryLock::acquire(&registry.with_extension("lock"))?;
    let mut installed: serde_json::Value = match std::fs::read_to_string(registry) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", registry.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({"version": 1, "plugins": []})
        }
        Err(error) => return Err(format!("cannot read {}: {error}", registry.display())),
    };
    let records = installed["plugins"]
        .as_array_mut()
        .ok_or_else(|| format!("{} has no plugins array", registry.display()))?;
    for plugin in plugins {
        install_launch_files(&plugin.files, Path::new("/"))?;
        records.retain(|record| record["id"] != plugin.id);
        records.push(serde_json::json!({
            "id": plugin.id,
            "root": plugin.root,
            "source": "local-path",
            "enabled": true,
            "installedAt": jiff::Timestamp::now().to_string(),
        }));
    }
    let temporary = registry.with_extension(format!("json.{}.tmp", std::process::id()));
    let bytes = serde_json::to_string_pretty(&installed)
        .map_err(|error| format!("cannot encode the plugin registry: {error}"))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, registry)
        .map_err(|error| format!("cannot replace {}: {error}", registry.display()))?;
    Ok(())
}

struct RegistryLock(PathBuf);

impl RegistryLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        for _ in 0..100 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => return Ok(Self(path.to_owned())),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(format!("cannot lock {}: {error}", path.display()));
                }
            }
        }
        Err(format!("timed out locking {}", path.display()))
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn env_num(key: &str) -> Option<i64> {
    std::env::var(key).ok()?.parse().ok()
}
