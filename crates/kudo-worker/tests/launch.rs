//! The launch plan: the CLI started with the worker's hooks and MCP server on
//! its command line, under the lease's session id, with the kickoff prompt.
//!
//! implements: b9ef8909cadf0092e9b9d01eb3fc3edbe9db4f2af17f80753571d9434628b1af@8fed628ae04360b98226c2ca157efd02ab98a1636e00ac9dab6d4384f2ad593e

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::path::{Path, PathBuf};

use kudo_context::ContextId;
use kudo_worker::{KICKOFF, WorkerSetup, new_lease_id, plan};
use serde_json::Value;

fn setup(lease: &str) -> WorkerSetup {
    WorkerSetup {
        store_dir: PathBuf::from("/state/contexts"),
        context: ContextId("ctx1".into()),
        lease: lease.into(),
        claim_ttl: 600,
        max_turns: 5,
        idle_limit: 0,
    }
}

fn flag<'a>(argv: &'a [String], name: &str) -> &'a str {
    let i = argv
        .iter()
        .position(|a| a == name)
        .unwrap_or_else(|| panic!("{name} in {argv:?}"));
    &argv[i + 1]
}

#[test]
fn the_cli_starts_under_the_lease_with_the_kickoff_prompt() {
    let lease = new_lease_id().unwrap();
    let p = plan(
        &setup(&lease),
        "claude",
        Path::new("/bin/kudo-shim"),
        Some("10.100.0.1:15400"),
        &[],
    );
    assert_eq!(p.argv[0], "claude");
    assert_eq!(flag(&p.argv, "--session-id"), lease);
    assert_eq!(p.argv.last().unwrap(), KICKOFF);
}

#[test]
fn the_settings_install_all_five_hooks_through_the_shim() {
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/opt/kudo shim/kudo-shim"),
        Some("h:1"),
        &[],
    );
    let settings: Value = serde_json::from_str(flag(&p.argv, "--settings")).unwrap();
    for (event, arg) in [
        ("SessionStart", "session-start"),
        ("PreToolUse", "pre-tool-use"),
        ("Stop", "stop"),
        ("PreCompact", "pre-compact"),
        ("UserPromptSubmit", "user-prompt-submit"),
    ] {
        let command = settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(settings["hooks"][event][0]["hooks"][0]["type"], "command");
        // A path with a space survives the shell the CLI runs hooks through.
        assert_eq!(
            command,
            format!("'/opt/kudo shim/kudo-shim' hook {arg}"),
            "{event}"
        );
    }
    // The Stop hook waits for work: its timeout covers the wait window.
    // implements: 9680c1e9f147ea4dba7d85a17e7c2405f6cd5b4bb13e87dd81849d986e06dfed@a7051e3e59de5e20f03e2117248f5af341ed55a104af661aca2026f28c4abc99
    let timeout = settings["hooks"]["Stop"][0]["hooks"][0]["timeout"]
        .as_i64()
        .unwrap_or(0);
    assert!(
        timeout >= kudo_worker::env::WAIT_WINDOW + 30,
        "Stop hook timeout {timeout}s"
    );
    let approval_timeout = settings["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"]
        .as_i64()
        .unwrap_or(0);
    assert!(
        approval_timeout > kudo_context::MCP_APPROVAL_TIMEOUT,
        "PreToolUse timeout {approval_timeout}s"
    );
}

#[test]
fn the_mcp_config_runs_the_shim_with_the_worker_environment() {
    let p = plan(
        &setup("lease-x"),
        "claude",
        Path::new("/bin/kudo-shim"),
        Some("h:1"),
        &[],
    );
    let mcp: Value = serde_json::from_str(flag(&p.argv, "--mcp-config")).unwrap();
    let server = &mcp["mcpServers"]["kudo"];
    assert_eq!(server["command"], "/bin/kudo-shim");
    assert_eq!(server["env"]["KUDO_HUB_ADDR"], "h:1");
    assert_eq!(server["env"]["KUDO_CONTEXT"], "ctx1");
    assert_eq!(server["env"]["KUDO_LEASE"], "lease-x");
}

#[test]
fn the_process_environment_carries_the_setup_for_the_hooks() {
    let p = plan(
        &setup("lease-x"),
        "claude",
        Path::new("/bin/kudo-shim"),
        Some("h:1"),
        &[],
    );
    let env: std::collections::HashMap<_, _> = p.env.into_iter().collect();
    let back = WorkerSetup::from_lookup(|k| env.get(k).cloned()).unwrap();
    assert_eq!(back, setup("lease-x"));
}

#[test]
fn lease_ids_are_distinct_v4_uuids() {
    let a = new_lease_id().unwrap();
    let b = new_lease_id().unwrap();
    assert_ne!(a, b);
    let groups: Vec<&str> = a.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "{a}"
    );
    assert!(groups[2].starts_with('4'), "version 4: {a}");
    assert!(
        matches!(&groups[3][..1], "8" | "9" | "a" | "b"),
        "RFC 4122 variant: {a}"
    );
    assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
}

/// Without a hub the worker still launches: no hub address is passed, and the
/// shim serves the worker tools on its own.
// implements: 482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae@482bd420c2a2251c6d5c2fa311e8a14b2bb0898cf84cedfbef88fe6ba8a119ae
#[test]
fn a_worker_launches_without_a_hub() {
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    let mcp: Value = serde_json::from_str(flag(&p.argv, "--mcp-config")).unwrap();
    assert!(
        mcp["mcpServers"]["kudo"]["env"]
            .get("KUDO_HUB_ADDR")
            .is_none()
    );
    assert!(p.env.iter().all(|(k, _)| k != "KUDO_HUB_ADDR"));
}

/// The context store's mirror reaches the worker's MCP server and its hooks
/// whatever the agent CLI passes on of its own environment.
#[test]
fn the_mirror_settings_are_forwarded_to_the_shim() {
    let forward = [
        ("KUDO_CONTEXT_MIRROR".to_owned(), "s3://b/p".to_owned()),
        (
            "AWS_ENDPOINT".to_owned(),
            "http://127.0.0.1:9100".to_owned(),
        ),
    ];
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &forward,
    );
    let mcp: Value = serde_json::from_str(flag(&p.argv, "--mcp-config")).unwrap();
    let env = &mcp["mcpServers"]["kudo"]["env"];
    assert_eq!(env["KUDO_CONTEXT_MIRROR"], "s3://b/p");
    assert_eq!(env["AWS_ENDPOINT"], "http://127.0.0.1:9100");
    assert!(p.env.contains(&forward[0]));
}

/// A node that says so runs its workers' CLI in bypass-permissions mode; any
/// other node, or any other value, leaves the CLI asking.
// implements: 1cc106dd91b0b36d535c15bec7fd446c3a46384519220c689786266e721661c8@3ab1a4e735b851802e773873ce516aaba9e5a5184f63c4b59ea373f03545b18a
#[test]
fn a_worker_bypasses_permissions_only_where_its_node_says_so() {
    use kudo_worker::env::{BYPASS_ENV, bypass_from};
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    assert!(
        !p.argv.iter().any(|a| a == "--permission-mode"),
        "{:?}",
        p.argv
    );
    let bypassing = p.clone().bypassing();
    assert_eq!(
        flag(&bypassing.argv, "--permission-mode"),
        "bypassPermissions"
    );
    assert_eq!(bypassing.argv[0], "claude");
    assert_eq!(bypassing.argv.last().unwrap(), KICKOFF);

    assert_eq!(BYPASS_ENV, "KUDO_WORKER_BYPASS");
    let given = |v: Option<&str>| {
        bypass_from(|k| (k == BYPASS_ENV).then(|| v.map(str::to_owned)).flatten())
    };
    assert!(given(Some("1")));
    assert!(!given(None));
    assert!(!given(Some("0")));
    assert!(!given(Some("yes")));
}

/// The launch pre-answers the CLI's first-run prompts: tips and non-essential
/// traffic (promotions) are off. (Project MCP servers: see below.)
// implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
#[test]
fn the_launch_preanswers_first_run_prompts() {
    let p = plan(
        &setup("lease-q"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    let settings: Value = serde_json::from_str(flag(&p.argv, "--settings")).unwrap();
    assert_eq!(settings["spinnerTipsEnabled"], Value::Bool(false));
    assert!(
        p.env
            .iter()
            .any(|(k, v)| k == "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC" && v == "1"),
        "{:?}",
        p.env
    );
    assert_eq!(p.argv.last().unwrap(), KICKOFF, "the kickoff stays last");
}

/// Shell calls are recorded as they return: a PostToolUse hook, for Bash
/// only, through the shim.
// implements: 6061a14e7f9b9eb2c7a3bbc0f5ff4f3778ceb08d71d46f8de4515f62ef3a2a0b@fdf56a326ad7b945b7ee31bb9503ee3365f629c5bf46347c3a8c970398fa65bd
#[test]
fn shell_calls_are_recorded_by_a_post_tool_use_hook() {
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    let settings: Value = serde_json::from_str(flag(&p.argv, "--settings")).unwrap();
    let entry = &settings["hooks"]["PostToolUse"][0];
    assert_eq!(entry["matcher"], "Bash");
    assert_eq!(
        entry["hooks"][0]["command"],
        "/bin/kudo-shim hook post-tool-use"
    );
}

/// A node that says so gives its workers kudo's MCP server only; any other
/// node, or any other value, leaves them the user's own servers too.
// implements: 130ca83438842f6cd1eeda1eafdda4a1cd34e9c5e9530587ea39bb85b02a06b9@57e52e9594073132eb79c8121ad1359e6c0c39325ff9fb1543752930262de155
#[test]
fn a_worker_sees_only_kudos_mcp_server_where_its_node_says_so() {
    use kudo_worker::env::{STRICT_MCP_ENV, strict_mcp_from};
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    assert!(
        !p.argv.iter().any(|a| a == "--strict-mcp-config"),
        "not strict unless the node says so: {:?}",
        p.argv
    );
    let strict = p.clone().strict_mcp();
    assert!(
        strict.argv.iter().any(|a| a == "--strict-mcp-config"),
        "{:?}",
        strict.argv
    );
    assert_eq!(strict.argv[0], "claude");
    assert_eq!(
        strict.argv.last().unwrap(),
        KICKOFF,
        "the kickoff stays last"
    );
    assert_eq!(
        flag(&strict.argv, "--mcp-config"),
        flag(&p.argv, "--mcp-config"),
        "kudo's own server either way"
    );

    assert_eq!(STRICT_MCP_ENV, "KUDO_WORKER_STRICT_MCP");
    let given = |v: Option<&str>| {
        strict_mcp_from(|k| {
            (k == STRICT_MCP_ENV)
                .then(|| v.map(str::to_owned))
                .flatten()
        })
    };
    assert!(given(Some("1")));
    assert!(!given(None));
    assert!(!given(Some("0")));
    assert!(!given(Some("true")));
}

fn repo_with(mcp_json: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kudo-launch-{}-{}",
        std::process::id(),
        new_lease_id().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(text) = mcp_json {
        std::fs::write(dir.join(".mcp.json"), text).unwrap();
    }
    dir
}

/// The servers a repository declares in its `.mcp.json`, by name; none when
/// it has no such file, or one that does not parse.
// implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
#[test]
fn a_repositorys_project_servers_are_read_by_name() {
    use kudo_worker::launch::project_servers;
    let repo = repo_with(Some(
        r#"{"mcpServers": {"tracker": {"command": "t"}, "search": {"url": "https://x"}}}"#,
    ));
    let mut names = project_servers(&repo);
    names.sort();
    assert_eq!(names, ["search", "tracker"]);
    assert!(project_servers(&repo_with(None)).is_empty());
    assert!(project_servers(&repo_with(Some("{not json"))).is_empty());
    assert!(project_servers(&repo_with(Some(r#"{"other": 1}"#))).is_empty());
}

/// Without strict mode, the repository's project servers are declined in the
/// settings the launch passes, so the CLI asks about none of them; the rest
/// of the settings stay as they were.
// implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
#[test]
fn project_servers_are_declined_without_a_prompt() {
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    let declined = p
        .clone()
        .declining(&["tracker".to_owned(), "search".to_owned()]);
    let settings: Value = serde_json::from_str(flag(&declined.argv, "--settings")).unwrap();
    assert_eq!(
        settings["disabledMcpjsonServers"],
        serde_json::json!(["tracker", "search"])
    );
    let before: Value = serde_json::from_str(flag(&p.argv, "--settings")).unwrap();
    assert_eq!(settings["hooks"], before["hooks"]);
    assert_eq!(settings["spinnerTipsEnabled"], Value::Bool(false));
    assert_eq!(declined.argv.last().unwrap(), KICKOFF);
    assert_eq!(declined.argv.len(), p.argv.len());

    // Nothing to decline: the settings are untouched.
    assert_eq!(p.clone().declining(&[]), p);
}

/// Project servers in any directory above the repository are declined too:
/// the CLI reads `.mcp.json` from the working directory and every parent.
// implements: 14129413411817d90fddd2bfff863c0f9a05dac9bef672a7aac7270676b2a697@9ee7f15ca68f69c0d1c7d7929f78d2b912a001f15d1b4cb0886096d3ae3d899a
#[test]
fn project_servers_above_the_repository_are_read_too() {
    use kudo_worker::launch::project_servers;
    let mono = repo_with(Some(r#"{"mcpServers": {"wiki": {"command": "w"}}}"#));
    let svc = mono.join("svc");
    std::fs::create_dir_all(&svc).unwrap();
    std::fs::write(
        svc.join(".mcp.json"),
        r#"{"mcpServers": {"tracker": {"command": "t"}, "wiki": {"command": "w2"}}}"#,
    )
    .unwrap();
    let mut names = project_servers(&svc);
    names.sort();
    assert_eq!(names, ["tracker", "wiki"], "each name once");
    let deeper = svc.join("a/b");
    std::fs::create_dir_all(&deeper).unwrap();
    let mut names = project_servers(&deeper);
    names.sort();
    assert_eq!(names, ["tracker", "wiki"]);
}

/// The node's choice, applied: strict where it says so (nothing declined, no
/// file read), otherwise the repository's project servers declined.
// implements: 130ca83438842f6cd1eeda1eafdda4a1cd34e9c5e9530587ea39bb85b02a06b9@57e52e9594073132eb79c8121ad1359e6c0c39325ff9fb1543752930262de155
#[test]
fn the_launcher_applies_the_nodes_mcp_choice_in_the_repository() {
    let p = plan(
        &setup("l"),
        "claude",
        Path::new("/bin/kudo-shim"),
        None,
        &[],
    );
    let repo = repo_with(Some(r#"{"mcpServers": {"tracker": {}}}"#));
    let strict = p.clone().for_node_mcp(true, &repo);
    assert_eq!(strict, p.clone().strict_mcp());
    let open = p.clone().for_node_mcp(false, &repo);
    assert_eq!(open, p.clone().declining(&["tracker".to_owned()]));
    assert!(!open.argv.iter().any(|a| a == "--strict-mcp-config"));
}
