//! How a worker session is started: the agent CLI with the worker's hooks and
//! the kudo MCP server passed on the command line, so a repo needs no setup of
//! its own and the repo's own settings are never touched.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e

use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{
    KICKOFF, LaunchFile, WorkerSetup,
    harness::{LaunchLink, LaunchPlugin},
};

/// The command line and environment that start one worker session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub files: Vec<LaunchFile>,
    pub plugins: Vec<LaunchPlugin>,
    pub links: Vec<LaunchLink>,
}

impl LaunchPlan {
    /// The same plan, with the CLI in bypass-permissions mode: its tools
    /// run unasked.
    // implements: 1cc106dd91b0b36d535c15bec7fd446c3a46384519220c689786266e721661c8@3ab1a4e735b851802e773873ce516aaba9e5a5184f63c4b59ea373f03545b18a
    pub fn bypassing(mut self) -> Self {
        let at = self.argv.len().min(1);
        self.argv.splice(
            at..at,
            [
                "--permission-mode".to_owned(),
                "bypassPermissions".to_owned(),
            ],
        );
        self
    }

    /// The same plan, with kudo's MCP server the only one the CLI loads.
    // implements: 130ca83438842f6cd1eeda1eafdda4a1cd34e9c5e9530587ea39bb85b02a06b9@57e52e9594073132eb79c8121ad1359e6c0c39325ff9fb1543752930262de155
    pub fn strict_mcp(mut self) -> Self {
        // Before the kickoff prompt, which stays last.
        let at = self.argv.len().saturating_sub(1);
        self.argv.insert(at, "--strict-mcp-config".to_owned());
        self
    }

    /// The same plan, with `servers` (a repository's project MCP servers)
    /// declined in its settings, so the CLI asks about none of them.
    // implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
    pub fn declining(mut self, servers: &[String]) -> Self {
        if servers.is_empty() {
            return self;
        }
        let Some(at) = self.argv.iter().position(|a| a == "--settings") else {
            return self;
        };
        let Some(text) = self.argv.get(at + 1) else {
            return self;
        };
        let Ok(mut settings) = serde_json::from_str::<Value>(text) else {
            return self;
        };
        settings["disabledMcpjsonServers"] = json!(servers);
        self.argv[at + 1] = settings.to_string();
        self
    }

    /// The same plan, with the node's MCP choice applied for a worker in
    /// `repo`: kudo's server only when `strict`, otherwise the user's servers
    /// as well, with the repository's project servers declined.
    pub fn for_node_mcp(self, strict: bool, repo: &Path) -> Self {
        if strict {
            self.strict_mcp()
        } else {
            self.declining(&project_servers(repo))
        }
    }
}

/// The MCP servers declared in a `.mcp.json` in `dir` or any directory
/// above it, by name, each once — the CLI reads them all as project servers.
/// A missing or unparsable file declares none (the CLI then has nothing from
/// it to ask about either).
// implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
pub fn project_servers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for at in dir.ancestors() {
        let Ok(text) = std::fs::read_to_string(at.join(".mcp.json")) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(servers) = parsed.get("mcpServers").and_then(Value::as_object) else {
            continue;
        };
        for name in servers.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// The hooks a worker installs, as (CLI event name, `kudo-shim hook` argument).
const HOOKS: [(&str, &str); 5] = [
    ("SessionStart", "session-start"),
    ("PreToolUse", "pre-tool-use"),
    ("Stop", "stop"),
    ("PreCompact", "pre-compact"),
    ("UserPromptSubmit", "user-prompt-submit"),
];

/// Plan a worker session: `cli` is the agent CLI, `shim` the kudo-shim binary
/// the hooks and the MCP server run as, `hub_addr` the hub the MCP server dials.
pub fn plan(
    setup: &WorkerSetup,
    cli: &str,
    shim: &Path,
    hub_addr: Option<&str>,
    forward: &[(String, String)],
) -> LaunchPlan {
    let shim_str = shim.display().to_string();

    let hooks: Map<String, Value> = HOOKS
        .iter()
        .map(|(event, arg)| {
            let command = format!("{} hook {arg}", shell_quote(&shim_str));
            let mut hook = json!({ "type": "command", "command": command });
            // The Stop hook waits for work: its timeout covers the window.
            // implements: 9680c1e9f147ea4dba7d85a17e7c2405f6cd5b4bb13e87dd81849d986e06dfed@a7051e3e59de5e20f03e2117248f5af341ed55a104af661aca2026f28c4abc99
            if *event == "Stop" {
                hook["timeout"] = json!(crate::env::WAIT_WINDOW + 60);
            } else if *event == "PreToolUse" {
                hook["timeout"] = json!(kudo_context::MCP_APPROVAL_TIMEOUT + 60);
            }
            ((*event).to_owned(), json!([{ "hooks": [hook] }]))
        })
        .collect();
    // An unattended pane has no person to answer a prompt: no tips (project
    // MCP servers: declined by the launcher, or strict — see `declining`).
    // implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
    let mut hooks = hooks;
    // Shell calls are recorded as they return, as citable evidence.
    // implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
    for (event, arg) in [
        ("PostToolUse", "post-tool-use"),
        ("PostToolUseFailure", "post-tool-use-failure"),
    ] {
        hooks.insert(
            event.to_owned(),
            json!([{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": format!("{} hook {arg}", shell_quote(&shim_str)),
                    "timeout": 30,
                }],
            }]),
        );
    }
    let settings = json!({ "hooks": hooks, "spinnerTipsEnabled": false });

    let mut server_env: Map<String, Value> = setup
        .to_env()
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    // Passed explicitly: what the agent CLI hands an MCP server of its own
    // environment is the CLI's business, and a worker must not lose its hub
    // or its store's mirror to it.
    let mut passed: Vec<(String, String)> = forward.to_vec();
    if let Some(addr) = hub_addr {
        passed.push(("KUDO_HUB_ADDR".to_owned(), addr.to_owned()));
    }
    for (k, v) in &passed {
        server_env.insert(k.clone(), Value::String(v.clone()));
    }
    let mcp = json!({
        "mcpServers": { "kudo": { "command": shim_str, "args": [], "env": server_env } }
    });

    let mut env = setup.to_env();
    env.extend(passed);
    // No promotions or feature-flag prompts in an unattended pane.
    env.push((
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_owned(),
        "1".to_owned(),
    ));

    LaunchPlan {
        files: Vec::new(),
        plugins: Vec::new(),
        links: Vec::new(),
        argv: vec![
            cli.to_owned(),
            "--settings".to_owned(),
            settings.to_string(),
            "--mcp-config".to_owned(),
            mcp.to_string(),
            "--session-id".to_owned(),
            setup.lease.clone(),
            KICKOFF.to_owned(),
        ],
        env,
    }
}

/// Quote for the POSIX shell the CLI runs hook commands through: bare when
/// safe, single-quoted otherwise.
fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+:=@".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// A fresh lease id: a random UUID (v4), since the CLI takes it as its session id.
pub fn new_lease_id() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}
