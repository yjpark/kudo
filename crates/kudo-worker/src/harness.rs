//! The boundary between the worker engine and one agent CLI.
//!
//! implements: de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019@de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019
//! task: 49fe78f15b4ec6be28ab78c0e7f8b982b97433bb5320ef8f14616eb1f89b6c2e@084b9203b79f0c48f48d6dc2634b4f9406878fd9f7663157217e264c987100b9

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{LaunchPlan, Stop, WorkerSetup};

mod kimi;
pub use kimi::KimiCode;

/// The environment variable selecting the agent harness.
pub const HARNESS_ENV: &str = "KUDO_AGENT_HARNESS";

/// What a harness can provide without imitating a different CLI.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HarnessCapabilities {
    pub interactive: bool,
    pub headless: bool,
    pub caller_session_id: bool,
    pub config_artifacts: bool,
    pub strict_mcp: bool,
    pub session_start_context: bool,
    pub post_tool_context: bool,
}

/// One configuration file a harness needs beside its launch command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFile {
    pub path: PathBuf,
    pub contents: String,
}

impl LaunchFile {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// A read-only link into a launcher's existing configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLink {
    pub source: PathBuf,
    pub target: PathBuf,
}

/// A plugin and the files that make it available to an agent CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlugin {
    pub id: String,
    pub root: PathBuf,
    pub files: Vec<LaunchFile>,
}

/// The worker mode a launch must provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Interactive,
    Headless,
}

/// The node and repository choices applied to a launch.
#[derive(Debug, Clone, Copy)]
pub struct LaunchOptions<'a> {
    pub mode: LaunchMode,
    pub bypass: bool,
    pub strict_mcp: bool,
    pub repo: &'a Path,
}

/// A shell call in the harness-neutral evidence format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCall {
    pub event: String,
    pub command: String,
    pub output: String,
    pub exit: Option<i32>,
}

/// A PreToolUse decision, independent of the agent CLI's wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolDecision {
    Allow,
    Deny(String),
}

/// What a hook prints and how it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReply {
    pub stdout: String,
    pub code: i32,
}

impl HookReply {
    pub fn quiet() -> Self {
        Self {
            stdout: String::new(),
            code: 0,
        }
    }

    pub fn json(value: &Value) -> Self {
        Self {
            stdout: value.to_string(),
            code: 0,
        }
    }

    /// A failure the CLI shows the human (exit code 1 is non-blocking).
    pub fn failed(message: impl std::fmt::Display) -> Self {
        Self {
            stdout: String::new(),
            code: 1,
        }
        .with_stderr(message)
    }

    pub fn with_stderr(self, message: impl std::fmt::Display) -> Self {
        eprintln!("kudo-shim hook: {message}");
        self
    }
}

/// CLI-specific launch and hook behavior, kept out of the worker engine.
pub trait AgentHarness {
    fn capabilities(&self) -> HarnessCapabilities;

    fn launch(
        &self,
        setup: &WorkerSetup,
        cli: Option<&str>,
        shim: &Path,
        hub_addr: Option<&str>,
        forward: &[(String, String)],
        options: LaunchOptions<'_>,
    ) -> Result<LaunchPlan, String>;

    fn user_prompt<'a>(&self, input: &'a Value) -> Option<&'a str>;

    fn user_prompt_reply(&self, _context: Option<&str>) -> HookReply {
        HookReply::quiet()
    }

    fn shell_call(&self, input: &Value) -> Option<ShellCall>;

    fn quiet_reply(&self) -> HookReply {
        HookReply::quiet()
    }

    fn session_start_reply(&self, context: &str) -> HookReply;

    fn stop_reply(&self, stop: &Stop) -> HookReply;

    fn post_tool_reply(&self, event: &str, context: &str) -> HookReply;

    fn pre_tool_reply(&self, decision: &PreToolDecision) -> HookReply {
        let (permission, reason) = match decision {
            PreToolDecision::Allow => ("allow", "allowed by kudo"),
            PreToolDecision::Deny(reason) => ("deny", reason.as_str()),
        };
        HookReply::json(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": permission,
                "permissionDecisionReason": reason,
            }
        }))
    }
}

/// Claude Code's launch and hook protocol.
pub struct ClaudeCode;

impl AgentHarness for ClaudeCode {
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            interactive: true,
            headless: true,
            caller_session_id: true,
            config_artifacts: false,
            strict_mcp: true,
            session_start_context: true,
            post_tool_context: true,
        }
    }

    fn launch(
        &self,
        setup: &WorkerSetup,
        cli: Option<&str>,
        shim: &Path,
        hub_addr: Option<&str>,
        forward: &[(String, String)],
        options: LaunchOptions<'_>,
    ) -> Result<LaunchPlan, String> {
        let capabilities = self.capabilities();
        let supported = match options.mode {
            LaunchMode::Interactive => capabilities.interactive,
            LaunchMode::Headless => capabilities.headless,
        };
        if !supported {
            return Err(format!(
                "Claude Code does not support {:?} workers",
                options.mode
            ));
        }
        let mut plan = crate::launch::plan(setup, cli.unwrap_or("claude"), shim, hub_addr, forward);
        if options.bypass {
            plan = plan.bypassing();
        }
        if options.mode == LaunchMode::Headless {
            let at = plan.argv.len().saturating_sub(1);
            plan.argv.insert(at, "-p".to_owned());
        }
        plan.env.push((HARNESS_ENV.to_owned(), "claude".to_owned()));
        Ok(plan.for_node_mcp(options.strict_mcp, options.repo))
    }

    fn user_prompt<'a>(&self, input: &'a Value) -> Option<&'a str> {
        input.get("prompt").and_then(Value::as_str)
    }

    fn shell_call(&self, input: &Value) -> Option<ShellCall> {
        if input.get("tool_name").and_then(Value::as_str) != Some("Bash") {
            return None;
        }
        let command = input
            .pointer("/tool_input/command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let response = input.get("tool_response").cloned().unwrap_or(Value::Null);
        let text = |key: &str| {
            response
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let mut output = match response.as_str() {
            Some(whole) => whole.to_owned(),
            None => text("stdout").to_owned(),
        };
        if !text("stderr").is_empty() {
            output.push_str("\n[stderr]\n");
            output.push_str(text("stderr"));
        }
        if let Some(error) = input.get("error").and_then(Value::as_str) {
            output.push_str("\n[error]\n");
            output.push_str(error);
        }
        let exit = ["exit_code", "exitCode", "returnCode", "code"]
            .iter()
            .find_map(|key| response.get(*key).and_then(Value::as_i64))
            .and_then(|code| i32::try_from(code).ok());
        Some(ShellCall {
            event: input
                .get("hook_event_name")
                .and_then(Value::as_str)
                .unwrap_or("PostToolUse")
                .to_owned(),
            command,
            output,
            exit,
        })
    }

    fn session_start_reply(&self, context: &str) -> HookReply {
        HookReply::json(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": context,
            }
        }))
    }

    fn stop_reply(&self, stop: &Stop) -> HookReply {
        match stop {
            Stop::Block { reason } => HookReply::json(&serde_json::json!({
                "decision": "block",
                "reason": reason,
            })),
            Stop::Allow(_) | Stop::Wait => HookReply::quiet(),
        }
    }

    fn post_tool_reply(&self, event: &str, context: &str) -> HookReply {
        HookReply::json(&serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": event,
                "additionalContext": context,
            }
        }))
    }
}

/// Select a harness explicitly; an absent choice preserves Claude as the
/// default during migration.
pub fn harness_from(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Box<dyn AgentHarness>, String> {
    match lookup(HARNESS_ENV).as_deref() {
        None | Some("claude") => Ok(Box::new(ClaudeCode)),
        Some("kimi") => Ok(Box::new(KimiCode)),
        Some(other) => Err(format!(
            "unknown agent harness {other:?}: supported harnesses are claude and kimi"
        )),
    }
}
