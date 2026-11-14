//! Kimi Code's launch and hook protocol.
//!
//! implements: de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019@de754b32b496489dcc9c584766662cc6149cb90c40e2a5095fa9c4da64e78019
//! task: 49fe78f15b4ec6be28ab78c0e7f8b982b97433bb5320ef8f14616eb1f89b6c2e@084b9203b79f0c48f48d6dc2634b4f9406878fd9f7663157217e264c987100b9

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::harness::{
    HARNESS_ENV, HarnessCapabilities, HookReply, LaunchFile, LaunchLink, LaunchMode, LaunchOptions,
    LaunchPlugin, ShellCall,
};
use crate::{AgentHarness, KICKOFF, LaunchPlan, Stop, WorkerSetup};

/// Kimi Code, using its own configuration and hook semantics.
pub struct KimiCode;

impl AgentHarness for KimiCode {
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            interactive: true,
            headless: true,
            caller_session_id: false,
            config_artifacts: true,
            strict_mcp: true,
            session_start_context: false,
            post_tool_context: false,
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
        if options.mode == LaunchMode::Headless && !options.bypass {
            return Err(
                "Kimi Code headless workers require KUDO_WORKER_BYPASS=1: print mode always \
                 uses auto permissions"
                    .to_owned(),
            );
        }
        if has_project_mcp(options.repo)? {
            return Err(
                "Kimi Code workers cannot decline .kimi-code/mcp.json while trusting the \
                 workspace; remove the project MCP servers or use Claude Code"
                    .to_owned(),
            );
        }

        let source_home = std::env::var("KIMI_CODE_HOME")
            .map(std::path::PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".kimi-code"))
                    .ok()
            })
            .ok_or_else(|| {
                "cannot find Kimi Code's data directory: set KIMI_CODE_HOME".to_owned()
            })?;
        let isolated_home = setup.store_dir.join("kimi-home").join(&setup.lease);
        let config = worker_config(&source_home.join("config.toml"))?;
        let links = vec![LaunchLink {
            source: source_home.join("credentials"),
            target: isolated_home.join("credentials"),
        }];
        let user_mcp = source_home.join("mcp.json");

        let mut passed = forward.to_vec();
        if let Some(addr) = hub_addr {
            passed.push(("KUDO_HUB_ADDR".to_owned(), addr.to_owned()));
        }
        passed.push((HARNESS_ENV.to_owned(), "kimi".to_owned()));
        passed.push((
            "KIMI_CODE_HOME".to_owned(),
            isolated_home.display().to_string(),
        ));
        passed.push(("KIMI_CODE_TRUST_WORKSPACE".to_owned(), "1".to_owned()));
        let mut env = setup.to_env();
        env.extend(passed.clone());
        let server_env: Map<String, Value> = env
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        let shim_text = shim.display().to_string();
        let command = |event: &str| format!("{} hook {event}", shell_quote(&shim_text));
        let mut hooks = vec![
            json!({"event": "SessionStart", "command": command("session-start")}),
            json!({"event": "SessionEnd", "command": command("session-end")}),
            json!({"event": "Stop", "command": command("stop"), "timeout": 600}),
            json!({"event": "PreCompact", "command": command("pre-compact")}),
            json!({"event": "UserPromptSubmit", "command": command("user-prompt-submit")}),
            json!({"event": "UserPromptQueued", "command": command("user-prompt-submit")}),
        ];
        for (event, arg) in [
            ("PostToolUse", "post-tool-use"),
            ("PostToolUseFailure", "post-tool-use-failure"),
        ] {
            hooks.push(json!({
                "event": event,
                "matcher": "Bash",
                "command": command(arg),
                "timeout": 30,
            }));
        }
        let server = json!({
            "command": shim_text,
            "args": [],
            "env": server_env,
        });
        let mut mcp = if options.strict_mcp {
            json!({"mcpServers": {}})
        } else {
            match std::fs::read_to_string(user_mcp) {
                Ok(text) => serde_json::from_str::<Value>(&text)
                    .map_err(|error| format!("cannot parse Kimi Code's mcp.json: {error}"))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    json!({"mcpServers": {}})
                }
                Err(error) => return Err(format!("cannot read Kimi Code's mcp.json: {error}")),
            }
        };
        let Some(servers) = mcp.get_mut("mcpServers").and_then(Value::as_object_mut) else {
            return Err("Kimi Code's mcp.json must contain an mcpServers object".to_owned());
        };
        servers.insert("kudo".to_owned(), server);
        let manifest = json!({
            "name": "kudo",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Kudo context workers and process tools",
            "hooks": hooks,
        });
        let root = isolated_home.join("plugins/managed/kudo");
        let plugin = LaunchPlugin {
            id: "kudo".to_owned(),
            root: root.clone(),
            files: vec![LaunchFile::new(
                root.join("kimi.plugin.json"),
                manifest.to_string(),
            )],
        };
        let cli = shell_quote(cli.unwrap_or("kimi"));
        let command = match options.mode {
            LaunchMode::Headless => format!(
                "{cli} --output-format stream-json -p {}\nstatus=$?\n{} hook session-end </dev/null\nexit $status",
                shell_quote(KICKOFF),
                shell_quote(&shim_text),
            ),
            LaunchMode::Interactive => {
                let cli = if options.bypass {
                    format!("{cli} --auto")
                } else {
                    cli
                };
                format!(
                    "{{ sleep 8; printf '%s' {}; sleep 1; printf '\\033[13u'; {} kimi-input; }} | script -q -e -c {} /dev/null\nstatus=$?\n{} hook session-end </dev/null\nexit $status",
                    shell_quote(KICKOFF),
                    shell_quote(&shim_text),
                    shell_quote(&cli),
                    shell_quote(&shim_text),
                )
            }
        };
        Ok(LaunchPlan {
            argv: vec!["sh".to_owned(), "-c".to_owned(), command],
            env,
            files: vec![
                LaunchFile::new(isolated_home.join("config.toml"), config),
                LaunchFile::new(isolated_home.join("mcp.json"), mcp.to_string()),
            ],
            plugins: vec![plugin],
            links,
        })
    }

    fn user_prompt<'a>(&self, input: &'a Value) -> Option<&'a str> {
        match input.get("prompt") {
            Some(Value::String(text)) => Some(text),
            Some(Value::Array(parts)) => parts
                .iter()
                .find_map(|part| part.get("text").and_then(Value::as_str)),
            _ => None,
        }
    }

    fn user_prompt_reply(&self, context: Option<&str>) -> HookReply {
        match context {
            Some(text) => HookReply {
                stdout: format!(
                    "{text}\n\nUse the kudo MCP tools directly: `mcp__kudo__pull`, \
                     `mcp__kudo__submit`, `mcp__kudo__evidence`, and `mcp__kudo__release`. \
                     Do not invoke `kudo` through Bash or speak JSON-RPC manually."
                ),
                code: 0,
            },
            None => HookReply::quiet(),
        }
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
        let response = input.get("tool_output").cloned().unwrap_or(Value::Null);
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

    fn session_start_reply(&self, _context: &str) -> HookReply {
        HookReply::quiet()
    }

    fn stop_reply(&self, stop: &Stop) -> HookReply {
        match stop {
            Stop::Block { reason } => HookReply {
                stdout: String::new(),
                code: 2,
            }
            .with_stderr(reason),
            Stop::Allow(_) | Stop::Wait => HookReply::quiet(),
        }
    }

    fn post_tool_reply(&self, _event: &str, _context: &str) -> HookReply {
        HookReply::quiet()
    }
}

fn has_project_mcp(repo: &Path) -> Result<bool, String> {
    let path = repo.join(".kimi-code/mcp.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let config: Value = serde_json::from_str(&text)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    Ok(config
        .get("mcpServers")
        .and_then(Value::as_object)
        .is_some_and(|servers| !servers.is_empty()))
}

fn worker_config(path: &Path) -> Result<String, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let manual = toml_setting(&source, None, "default_permission_mode", "\"manual\"");
    Ok(toml_setting(
        &manual,
        Some("workspace"),
        "require_trust",
        "true",
    ))
}

fn toml_setting(text: &str, section: Option<&str>, key: &str, value: &str) -> String {
    let setting = format!("{key} = {value}");
    let mut out = Vec::new();
    let mut selected = section.is_none();
    let mut saw_section = selected;
    let mut wrote = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if selected && !wrote {
                out.push(setting.clone());
                wrote = true;
            }
            let table = trimmed
                .strip_prefix('[')
                .and_then(|header| header.split_once(']'))
                .map(|(name, rest)| (name, rest.trim()));
            selected = section.is_some_and(|name| {
                table.is_some_and(|(candidate, rest)| {
                    candidate == name && (rest.is_empty() || rest.starts_with('#'))
                })
            });
            saw_section |= selected;
        }
        if selected
            && line
                .split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        {
            if !wrote {
                out.push(setting.clone());
                wrote = true;
            }
        } else {
            out.push(line.to_owned());
        }
    }
    if !wrote {
        if !saw_section && let Some(name) = section {
            out.push(format!("[{name}]"));
        }
        out.push(setting);
    }
    format!("{}\n", out.join("\n"))
}

fn shell_quote(text: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+:=@".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::toml_setting;

    #[test]
    fn toml_table_headers_may_have_comments() {
        let out = toml_setting(
            "[workspace] # local policy\nrequire_trust = false\n",
            Some("workspace"),
            "require_trust",
            "true",
        );
        assert_eq!(out.matches("[workspace]").count(), 1, "{out}");
        assert!(out.contains("require_trust = true"), "{out}");
        assert!(!out.contains("require_trust = false"), "{out}");
    }
}
