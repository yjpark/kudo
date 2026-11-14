//! Kimi Code harness conformance, from real 2.5.2 payloads.
//!
//! implements: de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019@de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::path::Path;

use kudo_context::ContextId;
use kudo_worker::harness::{HARNESS_ENV, LaunchMode};
use kudo_worker::{
    AgentHarness, KICKOFF, KimiCode, LaunchOptions, Stop, WorkerSetup, harness_from,
};
use serde_json::Value;

fn setup() -> WorkerSetup {
    WorkerSetup {
        store_dir: "/state/contexts".into(),
        context: ContextId("ctx1".into()),
        lease: "lease-1".into(),
        claim_ttl: 600,
        max_turns: 5,
        idle_limit: 0,
    }
}

#[test]
fn kimi_is_selected_explicitly_and_reports_headless_and_interactive_support() {
    let harness = harness_from(|key| {
        (key == HARNESS_ENV)
            .then(|| Some("kimi".to_owned()))
            .flatten()
    })
    .unwrap();
    let capabilities = harness.capabilities();
    assert!(capabilities.interactive);
    assert!(capabilities.headless);
    assert!(!capabilities.caller_session_id);
    assert!(capabilities.config_artifacts);
    assert!(capabilities.strict_mcp);
    assert!(!capabilities.session_start_context);
    assert!(!capabilities.post_tool_context);
}

#[test]
fn kimi_headless_launch_uses_print_mode_and_a_kudo_plugin() {
    let forward = [
        ("KUDO_CONTEXT_MIRROR".to_owned(), "s3://b/p".to_owned()),
        ("AWS_REGION".to_owned(), "auto".to_owned()),
    ];
    let plan = KimiCode
        .launch(
            &setup(),
            None,
            Path::new("/bin/kudo-shim"),
            Some("hub:15400"),
            &forward,
            LaunchOptions {
                mode: LaunchMode::Headless,
                bypass: true,
                strict_mcp: false,
                repo: Path::new("/tmp"),
            },
        )
        .unwrap();
    assert_eq!(plan.argv[0], "sh");
    assert_eq!(plan.argv[1], "-c");
    let command = &plan.argv[2];
    assert!(command.contains("kimi"));
    assert!(command.contains("--output-format stream-json"));
    assert!(command.contains("-p"));
    assert!(command.contains(KICKOFF));
    assert!(command.contains("hook session-end"));
    assert!(command.contains("exit $status"));
    assert!(!plan.argv.iter().any(|arg| arg == "--settings"));
    assert!(!plan.argv.iter().any(|arg| arg == "--mcp-config"));
    assert!(!plan.argv.iter().any(|arg| arg == "--session-id"));
    assert!(
        plan.env
            .iter()
            .any(|(key, value)| key == HARNESS_ENV && value == "kimi")
    );

    let isolated_home = plan
        .env
        .iter()
        .find(|(key, _)| key == "KIMI_CODE_HOME")
        .map(|(_, value)| value.as_str())
        .unwrap();
    assert_eq!(isolated_home, "/state/contexts/kimi-home/lease-1");
    assert!(
        !plan.links.iter().any(|link| {
            link.target == Path::new("/state/contexts/kimi-home/lease-1/config.toml")
        })
    );
    assert!(
        plan.links.iter().any(|link| {
            link.target == Path::new("/state/contexts/kimi-home/lease-1/credentials")
        })
    );

    assert_eq!(plan.plugins.len(), 1);
    let plugin = &plan.plugins[0];
    assert!(plugin.root.starts_with(isolated_home));
    assert_eq!(plugin.id, "kudo");
    assert_eq!(plugin.files.len(), 1);
    let manifest: Value = serde_json::from_str(&plugin.files[0].contents).unwrap();
    assert!(manifest.get("mcpServers").is_none());
    assert_eq!(plan.files.len(), 2);
    let mcp_file = plan
        .files
        .iter()
        .find(|file| file.path == Path::new(&format!("{isolated_home}/mcp.json")))
        .unwrap();
    let mcp: Value = serde_json::from_str(&mcp_file.contents).unwrap();
    let server = &mcp["mcpServers"]["kudo"];
    assert_eq!(server["command"], "/bin/kudo-shim");
    assert_eq!(server["env"]["KUDO_CONTEXT"], "ctx1");
    assert_eq!(server["env"]["KUDO_LEASE"], "lease-1");
    assert_eq!(server["env"]["KUDO_HUB_ADDR"], "hub:15400");
    assert_eq!(server["env"]["KUDO_CONTEXT_MIRROR"], "s3://b/p");
    assert_eq!(server["env"]["AWS_REGION"], "auto");
    assert_eq!(server["env"][HARNESS_ENV], "kimi");
    for event in [
        "SessionStart",
        "SessionEnd",
        "Stop",
        "PreCompact",
        "UserPromptSubmit",
        "UserPromptQueued",
        "PostToolUse",
        "PostToolUseFailure",
    ] {
        assert!(
            manifest["hooks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hook| hook["event"] == event),
            "missing {event}: {manifest}"
        );
    }
    let session_end = manifest["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hook| hook["event"] == "SessionEnd")
        .unwrap();
    assert!(
        session_end["command"]
            .as_str()
            .unwrap()
            .ends_with("hook session-end")
    );
}

#[test]
fn kimi_interactive_launch_uses_a_pty_and_controls_permissions() {
    for bypass in [false, true] {
        let plan = KimiCode
            .launch(
                &setup(),
                None,
                Path::new("/bin/kudo-shim"),
                None,
                &[],
                LaunchOptions {
                    mode: LaunchMode::Interactive,
                    bypass,
                    strict_mcp: false,
                    repo: Path::new("/tmp"),
                },
            )
            .unwrap();
        assert_eq!(plan.argv[0], "sh");
        assert_eq!(plan.argv[1], "-c");
        let command = &plan.argv[2];
        assert!(command.contains("sleep 8"), "{command}");
        assert!(command.contains("printf"), "{command}");
        assert!(command.contains("printf '%s'"), "{command}");
        assert!(command.contains(r"sleep 1; printf '\033[13u'"), "{command}");
        assert!(command.contains("kimi-input"), "{command}");
        assert!(command.contains("script -q -e -c"), "{command}");
        assert!(command.contains(KICKOFF), "{command}");
        assert_eq!(command.contains("--auto"), bypass, "{command}");
        assert!(!command.contains("--output-format"), "{command}");
        assert!(
            plan.env
                .iter()
                .any(|(key, value)| key == "KIMI_CODE_TRUST_WORKSPACE" && value == "1")
        );

        let config = plan
            .files
            .iter()
            .find(|file| file.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            config
                .contents
                .contains("default_permission_mode = \"manual\"")
        );
        assert!(
            !config
                .contents
                .contains("default_permission_mode = \"auto\"")
        );
        assert!(config.contents.contains("require_trust = true"));
    }
}

#[test]
fn kimi_strict_mcp_loads_only_kudo() {
    let plan = KimiCode
        .launch(
            &setup(),
            None,
            Path::new("/bin/kudo-shim"),
            None,
            &[],
            LaunchOptions {
                mode: LaunchMode::Headless,
                bypass: true,
                strict_mcp: true,
                repo: Path::new("/tmp"),
            },
        )
        .unwrap();
    let mcp_file = plan
        .files
        .iter()
        .find(|file| file.path.ends_with("mcp.json"))
        .unwrap();
    let mcp: Value = serde_json::from_str(&mcp_file.contents).unwrap();
    let servers = mcp["mcpServers"].as_object().unwrap();
    assert_eq!(servers.keys().collect::<Vec<_>>(), ["kudo"]);
}

#[test]
fn kimi_refuses_project_mcp_instead_of_inheriting_workspace_trust() {
    let repo = std::env::temp_dir().join(format!("kudo-kimi-project-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join(".kimi-code")).unwrap();
    std::fs::write(
        repo.join(".kimi-code/mcp.json"),
        r#"{"mcpServers":{"other":{"command":"other"}}}"#,
    )
    .unwrap();
    for strict_mcp in [false, true] {
        let error = KimiCode
            .launch(
                &setup(),
                None,
                Path::new("/bin/kudo-shim"),
                None,
                &[],
                LaunchOptions {
                    mode: LaunchMode::Interactive,
                    bypass: true,
                    strict_mcp,
                    repo: &repo,
                },
            )
            .unwrap_err();
        assert!(error.contains(".kimi-code/mcp.json"), "{error}");
    }
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn kimi_headless_without_bypass_is_refused() {
    let error = KimiCode
        .launch(
            &setup(),
            None,
            Path::new("/bin/kudo-shim"),
            None,
            &[],
            LaunchOptions {
                mode: LaunchMode::Headless,
                bypass: false,
                strict_mcp: false,
                repo: Path::new("/tmp"),
            },
        )
        .unwrap_err();
    assert!(error.contains("KUDO_WORKER_BYPASS=1"));
}

#[test]
fn kimi_uses_plain_prompt_context_and_exit_two_for_stop() {
    let startup = KimiCode.user_prompt_reply(Some("the head"));
    assert!(startup.stdout.starts_with("the head\n\n"));
    for tool in [
        "mcp__kudo__pull",
        "mcp__kudo__submit",
        "mcp__kudo__evidence",
        "mcp__kudo__release",
    ] {
        assert!(startup.stdout.contains(tool), "missing {tool}");
    }
    assert!(startup.stdout.contains("Do not invoke `kudo` through Bash"));
    assert!(startup.stdout.contains("speak JSON-RPC manually"));
    assert_eq!(startup.code, 0);
    assert!(KimiCode.user_prompt_reply(None).stdout.is_empty());
    assert!(KimiCode.session_start_reply("ignored").stdout.is_empty());

    let next = KimiCode.stop_reply(&Stop::Block {
        reason: "next item".into(),
    });
    assert_eq!(next.code, 2);
    assert!(next.stdout.is_empty());
    assert_eq!(KimiCode.stop_reply(&Stop::Wait).code, 0);
}

#[test]
fn kimi_normalizes_content_array_prompts_and_tool_output() {
    let prompt = serde_json::json!({
        "prompt": [{"type": "text", "text": "take over"}]
    });
    assert_eq!(KimiCode.user_prompt(&prompt), Some("take over"));

    let call = KimiCode
        .shell_call(&serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "printf PROBE"},
            "tool_output": "PROBE"
        }))
        .unwrap();
    assert_eq!(call.command, "printf PROBE");
    assert_eq!(call.output, "PROBE");
    assert_eq!(call.event, "PostToolUse");

    let failed = KimiCode
        .shell_call(&serde_json::json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_output": "failed",
            "error": "Exit code 1"
        }))
        .unwrap();
    assert_eq!(failed.output, "failed\n[error]\nExit code 1");
}
