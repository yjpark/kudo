//! `kudo-shim context new | enqueue | show`: making and driving a context by
//! hand, before the console exists.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use kudo_shim::cli;

const T0: i64 = 1_792_500_000;

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn new_creates_the_store_and_prints_the_context_id() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let id = cli::context(&dir, &args(&["new", "dogfood"]), T0).unwrap();
    let id = id.trim();
    assert_eq!(id.len(), 64, "a kino id: {id}");

    cli::context(&dir, &args(&["enqueue", id, "write the first test"]), T0).unwrap();
    cli::context(&dir, &args(&["enqueue", id, "make it pass"]), T0).unwrap();
    let shown = cli::context(&dir, &args(&["show", id]), T0).unwrap();
    assert!(shown.contains("# dogfood"), "{shown}");
    assert!(shown.contains("2 ready"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

// implements: a99975ca76ea8d354544294d35af3d49c1215a2029be4aa91f51e541fc26fdc7
#[test]
fn enqueue_after_makes_the_named_item_a_dependency() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-after-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "dependent"]), T0).unwrap();
    let ctx = ctx.trim();
    let first = cli::context(&dir, &args(&["enqueue", ctx, "first"]), T0).unwrap();
    let first = first.trim().strip_prefix("enqueued ").unwrap();
    cli::context(
        &dir,
        &args(&["enqueue", ctx, "--after", first, "second"]),
        T0,
    )
    .unwrap();

    let shown = cli::context(&dir, &args(&["show", ctx]), T0).unwrap();
    assert!(shown.contains("1 ready"), "{shown}");
    assert!(shown.contains("1 waiting"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_command_says_how_to_use_it() {
    let dir = std::env::temp_dir();
    let err = cli::context(&dir, &args(&["frobnicate"]), T0).unwrap_err();
    assert!(
        err.contains("new")
            && err.contains("enqueue")
            && err.contains("show")
            && err.contains("list")
            && err.contains("repo add"),
        "{err}"
    );
}

#[test]
fn list_shows_each_context_with_its_counts_and_a_prefix_names_one() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let listed = cli::context(&dir, &args(&["list"]), T0).unwrap();
    assert!(listed.contains("no contexts"), "{listed}");

    let a = cli::context(&dir, &args(&["new", "first", "one"]), T0).unwrap();
    let b = cli::context(&dir, &args(&["new", "second"]), T0 + 60).unwrap();
    let (a, b) = (a.trim(), b.trim());

    // A short prefix stands for the full id.
    cli::context(&dir, &args(&["enqueue", &b[..8], "an ask"]), T0 + 61).unwrap();
    let shown = cli::context(&dir, &args(&["show", &b[..8]]), T0 + 62).unwrap();
    assert!(shown.contains("1 ready"), "{shown}");

    let listed = cli::context(&dir, &args(&["list"]), T0 + 62).unwrap();
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 2, "{listed}");
    assert!(lines[0].starts_with(&b[..12]), "newest first: {listed}");
    assert!(
        lines[0].contains("second") && lines[0].contains("1 ready"),
        "{listed}"
    );
    assert!(
        lines[1].starts_with(&a[..12]) && lines[1].contains("first one"),
        "{listed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// implements: b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9@b53b7abd832f2cd0e58e5084f5733af5d56f762c52cd6fd404913911920a6de9
#[test]
fn repo_add_declares_a_repo_and_show_lists_it() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "spans"]), T0).unwrap();
    let ctx = ctx.trim();

    let said = cli::context(
        &dir,
        &args(&["repo", "add", &ctx[..8], "kudo", "/w/kudo", "--node", "n1"]),
        T0,
    )
    .unwrap();
    assert!(said.contains("kudo") && said.contains("n1"), "{said}");

    let shown = cli::context(&dir, &args(&["show", ctx]), T0).unwrap();
    assert!(shown.contains("repo kudo @ n1: /w/kudo"), "{shown}");

    let bad = cli::context(
        &dir,
        &args(&["repo", "add", ctx, "kudo", "rel/path", "--node", "n1"]),
        T0,
    );
    assert!(bad.is_err(), "{bad:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_worker_starts_in_a_declared_repo_on_this_node() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-wdir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "spans"]), T0).unwrap();
    let ctx = ctx.trim();
    let repo = dir.join("kudo-repo");
    std::fs::create_dir_all(&repo).unwrap();
    cli::context(
        &dir,
        &args(&[
            "repo",
            "add",
            ctx,
            "kudo",
            repo.to_str().unwrap(),
            "--node",
            "n1",
        ]),
        T0,
    )
    .unwrap();

    assert_eq!(
        cli::worker_dir(&dir, ctx, Some("kudo"), "n1").unwrap(),
        Some(repo.clone())
    );
    assert_eq!(
        cli::worker_dir_for_lease(
            &dir,
            ctx,
            Some("kudo"),
            "n1",
            "33333333-3333-4333-8333-333333333333",
        )
        .unwrap(),
        Some(repo.clone())
    );
    assert_eq!(cli::worker_dir(&dir, ctx, None, "n1").unwrap(), None);
    let err = cli::worker_dir(&dir, ctx, Some("kudo"), "n2").unwrap_err();
    assert!(err.contains("repo add"), "says how to declare it: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// implements: a6751902e8e01e5d7d5983196da9170f33f40d0d74246b5746977ea1c5e45098@a6751902e8e01e5d7d5983196da9170f33f40d0d74246b5746977ea1c5e45098
// implements: e45b144bd8c7b8efbc9ab9e0be43e549540b4da58859ba765a7ae53567349e34
#[test]
fn an_isolated_context_gives_each_lease_a_persistent_worktree_off_main() {
    let root = std::env::temp_dir().join(format!("kudo-shim-worktree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = root.join("store");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |dir: &std::path::Path, argv: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(argv)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{argv:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .arg(&repo)
        .output()
        .unwrap();
    git(&repo, &["config", "user.email", "kudo@example.org"]);
    git(&repo, &["config", "user.name", "Kudo Test"]);
    std::fs::write(repo.join("tracked"), "main\n").unwrap();
    git(&repo, &["add", "tracked"]);
    git(&repo, &["commit", "-m", "main"]);
    let main = git(&repo, &["rev-parse", "HEAD"]);

    let ctx = cli::context(&store, &args(&["new", "--worktree", "isolated"]), T0).unwrap();
    let ctx = ctx.trim();
    cli::context(
        &store,
        &args(&[
            "repo",
            "add",
            ctx,
            "kudo",
            repo.to_str().unwrap(),
            "--node",
            "n1",
        ]),
        T0,
    )
    .unwrap();
    let lease_a = "11111111-1111-4111-8111-111111111111";
    let lease_b = "22222222-2222-4222-8222-222222222222";
    let a = cli::worker_dir_for_lease(&store, ctx, Some("kudo"), "n1", lease_a)
        .unwrap()
        .unwrap();
    let b = cli::worker_dir_for_lease(&store, ctx, Some("kudo"), "n1", lease_b)
        .unwrap()
        .unwrap();

    assert_ne!(a, b);
    assert_ne!(a, repo);
    assert_eq!(
        std::fs::read_to_string(a.join("tracked")).unwrap(),
        "main\n"
    );
    assert_eq!(git(&a, &["rev-parse", "HEAD"]), main);
    std::fs::write(a.join("unfinished"), "keep me\n").unwrap();
    let again = cli::worker_dir_for_lease(&store, ctx, Some("kudo"), "n1", lease_a)
        .unwrap()
        .unwrap();
    assert_eq!(again, a);
    assert_eq!(
        std::fs::read_to_string(a.join("unfinished")).unwrap(),
        "keep me\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn trace_lists_the_loop_events_and_why_names_what_wrote_the_head() {
    use kudo_context::{ContextId, ContextStore, Submission, TraceEvent};
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-trace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "traced"]), T0).unwrap();
    let ctx = ctx.trim().to_owned();

    let why = cli::context(&dir, &args(&["why", &ctx]), T0).unwrap();
    assert!(why.contains("no submission"), "the first head: {why}");

    let store = ContextStore::open(&dir).unwrap();
    let id = ContextId(ctx.clone());
    store.enqueue(&id, "the ask", T0).unwrap();
    let claim = store.pull(&id, "lease-abc", T0, 600).unwrap().unwrap();
    store
        .record_trace(&id, "lease-abc", &TraceEvent::Pull(claim.id.clone()), T0)
        .unwrap();
    let sub = Submission {
        head: "# traced\nanswered\n".into(),
        log: "answered".into(),
        ..Default::default()
    };
    let done = store.submit(&claim.id, &sub, T0 + 1).unwrap();

    let trace = cli::context(&dir, &args(&["trace", &ctx[..8]]), T0 + 2).unwrap();
    assert!(
        trace.contains("lease-ab") && trace.contains("pull"),
        "{trace}"
    );
    assert!(trace.contains(&claim.id.0[..12]), "{trace}");

    let why = cli::context(&dir, &args(&["why", &ctx]), T0 + 2).unwrap();
    for part in [&done.id.0[..12], &claim.id.0[..12], "the ask", "lease-abc"] {
        assert!(why.contains(part), "{part} in {why}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_declared_repo_missing_on_this_node_is_named_in_the_error() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "gone"]), T0).unwrap();
    let ctx = ctx.trim();
    cli::context(
        &dir,
        &args(&[
            "repo",
            "add",
            ctx,
            "ghost",
            "/no/such/dir/anywhere",
            "--node",
            "n1",
        ]),
        T0,
    )
    .unwrap();
    let err = cli::worker_dir(&dir, ctx, Some("ghost"), "n1").unwrap_err();
    assert!(
        err.contains("ghost") && err.contains("/no/such/dir/anywhere"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// With a mirror, a machine that has never held the store opens it and
/// finds what another machine wrote.
// implements: fc473f56b1b8bdbae4a31e54e7281c4aeed3c08bd03804b256ca37e1937fe66f@61632ce8ff602a5a85acd48cc9208e1ca388d7a696ce46d8459e27b85426538e
#[test]
fn a_mirrored_store_opens_on_a_new_machine_with_what_the_other_wrote() {
    let root = std::env::temp_dir().join(format!("kudo-shim-cli-mirror-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let url = format!("file://{}", root.join("bucket").display());

    let pc = cli::open_store(&root.join("pc"), Some(&url), true).unwrap();
    let ctx = pc.create_context("crosses machines", "# x\n", T0).unwrap();

    let edger = cli::open_store(&root.join("edger"), Some(&url), true).unwrap();
    let seen: Vec<_> = edger
        .contexts()
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(seen, std::slice::from_ref(&ctx));

    assert!(
        cli::open_store(&root.join("nowhere"), None, true).is_err(),
        "without a mirror, a missing store is an error"
    );
    assert!(
        cli::open_store(&root.join("typo"), Some(&url), false).is_err(),
        "a hook never creates a store, mirror or not"
    );
    assert!(!root.join("typo").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// `context list` on a machine that never held the store, with a mirror,
/// lists what the other machine created.
#[test]
fn list_on_a_new_machine_with_a_mirror_shows_the_other_machines_contexts() {
    let root =
        std::env::temp_dir().join(format!("kudo-shim-cli-list-mirror-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let url = format!("file://{}", root.join("bucket").display());
    let made = cli::context_with(
        &root.join("pc"),
        Some(&url),
        None,
        &args(&["new", "made on pc"]),
        T0,
    )
    .unwrap();
    let listed =
        cli::context_with(&root.join("edger"), Some(&url), None, &args(&["list"]), T0).unwrap();
    assert!(listed.starts_with(&made.trim()[..12]), "{listed}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_empty_ask_is_refused() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = cli::context(&dir, &args(&["new", "e"]), T0).unwrap();
    assert!(cli::context(&dir, &args(&["enqueue", ctx.trim(), "   "]), T0).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A launcher that picked the lease (the console, to record where it runs)
/// hands it in; otherwise a fresh one is made.
#[test]
fn a_worker_takes_the_lease_it_is_given() {
    let given = "6f1d0c9e-2b7a-4c3e-9f00-1234567890ab";
    assert_eq!(cli::lease_for(Some(given.into())).unwrap(), given);
    let fresh = cli::lease_for(None).unwrap();
    assert_eq!(fresh.len(), 36);
    assert!(cli::lease_for(Some("not/a lease".into())).is_err());
}

/// A due item and a check are enqueued by hand, wait for their time, and
/// show as not yet due; a context's wake node is declared and shown.
// implements: 716432a845eb22babf957938aaa9a9c0f3cd3bea3ebee24c89a5147a795e7e2b@80a3c962223cc4b37ffceccfdfb83cb9d143fd3e956d4cbe19c7ec3e93774ffd
// implements: c11043084f97c61ff3f41e4d07fb2361aa871213021e9c15d0e4cee37c99bde1
// implements: 4afb77c56fe120a92c8ecb4f01b9a8873e9a41fb8eecb450b9a60e1b1ec327c0@550531c07103cb27e03f2d4362bcb4ed367debf0a67293484865999f716c77a4
#[test]
fn due_items_checks_and_the_wake_node_by_hand() {
    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-due-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = cli::context(&dir, &args(&["new", "home lab"]), T0).unwrap();
    let id = id.trim();

    cli::context(
        &dir,
        &args(&["enqueue", id, "--due", "+1h", "rotate the key"]),
        T0,
    )
    .unwrap();
    cli::context(
        &dir,
        &args(&[
            "check",
            id,
            "the backup ran",
            "--pass",
            "a snapshot newer than 26 hours",
            "--window",
            "the last 26 hours",
            "--due",
            "+2h",
        ]),
        T0,
    )
    .unwrap();
    cli::context(&dir, &args(&["wake-node", id, "yolo"]), T0).unwrap();

    let shown = cli::context(&dir, &args(&["show", id]), T0).unwrap();
    assert!(shown.contains("0 ready"), "{shown}");
    assert!(shown.contains("2 not yet due"), "{shown}");
    assert!(shown.contains("wakes on yolo"), "{shown}");

    let later = cli::context(&dir, &args(&["show", id]), T0 + 2 * 3600).unwrap();
    assert!(later.contains("2 ready"), "{later}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// When, as typed: `+30m`, `+2h`, `+1d`, or an RFC 3339 time; anything else is
/// refused, as is a check without a pass condition.
#[test]
fn a_due_time_is_relative_or_absolute_and_a_check_needs_its_condition() {
    assert_eq!(cli::parse_when("+30m", T0), Ok(T0 + 1800));
    assert_eq!(cli::parse_when("+2h", T0), Ok(T0 + 7200));
    assert_eq!(cli::parse_when("+1d", T0), Ok(T0 + 86_400));
    assert_eq!(
        cli::parse_when("2026-10-21T10:00:00Z", T0),
        Ok(1_792_576_800)
    );
    assert!(cli::parse_when("soon", T0).is_err());
    assert!(cli::parse_when("+5y", T0).is_err());
    assert!(cli::parse_when("+0m", T0).is_err());
    assert!(cli::parse_when("+-5m", T0).is_err());
    assert!(
        cli::parse_when("+5é", T0).is_err(),
        "no panic on a multibyte unit"
    );
    assert!(cli::parse_when("+", T0).is_err());

    let dir = std::env::temp_dir().join(format!("kudo-shim-cli-chk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = cli::context(&dir, &args(&["new", "lab"]), T0).unwrap();
    assert!(cli::context(&dir, &args(&["check", id.trim(), "the backup ran"]), T0).is_err());
    assert!(cli::context(&dir, &args(&["enqueue", id.trim(), "--due", "+1h"]), T0).is_err());
    assert!(cli::context(&dir, &args(&["enqueue", id.trim(), "--due"]), T0).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn launch_artifacts_land_where_the_worker_runs() {
    let root = std::env::temp_dir().join(format!("kudo-launch-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let absolute = root.join("absolute.json");
    let files = vec![
        kudo_worker::LaunchFile::new(".agent/nested/settings.json", "relative"),
        kudo_worker::LaunchFile::new(&absolute, "absolute"),
    ];
    cli::install_launch_files(&files, &repo).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join(".agent/nested/settings.json")).unwrap(),
        "relative"
    );
    assert_eq!(std::fs::read_to_string(&absolute).unwrap(), "absolute");
    use std::os::unix::fs::PermissionsExt;
    for path in [repo.join(".agent/nested/settings.json"), absolute] {
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{} is private", path.display());
    }

    let blocked = root.join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    let error = cli::install_launch_files(
        &[kudo_worker::LaunchFile::new(&blocked, "not a file")],
        &repo,
    )
    .unwrap_err();
    assert!(error.contains("blocked"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn launch_plugins_merge_with_the_existing_registry() {
    let root = std::env::temp_dir().join(format!("kudo-launch-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let registry = root.join("installed.json");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        &registry,
        r#"{"version":1,"plugins":[{"id":"other","root":"/other","source":"local-path","enabled":true,"installedAt":"2026-01-01T00:00:00.000Z"}]}"#,
    )
    .unwrap();
    let plugin_root = root.join("kudo");
    let plugin = kudo_worker::harness::LaunchPlugin {
        id: "kudo".into(),
        root: plugin_root.clone(),
        files: vec![kudo_worker::LaunchFile::new(
            plugin_root.join("kimi.plugin.json"),
            r#"{"name":"kudo"}"#,
        )],
    };
    cli::install_launch_plugins(std::slice::from_ref(&plugin), &registry).unwrap();
    let installed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
    let plugins = installed["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 2, "{installed}");
    assert!(plugins.iter().any(|p| p["id"] == "other"));
    assert!(plugins.iter().any(|p| p["id"] == "kudo"));
    assert_eq!(
        std::fs::read_to_string(plugin_root.join("kimi.plugin.json")).unwrap(),
        r#"{"name":"kudo"}"#
    );

    cli::install_launch_plugins(&[plugin], &registry).unwrap();
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
    assert_eq!(again["plugins"].as_array().unwrap().len(), 2, "{again}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn launch_links_are_idempotent_and_registry_merges_survive_concurrency() {
    let root = std::env::temp_dir().join(format!("kudo-launch-concurrent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.toml");
    std::fs::write(&source, "model = 'test'").unwrap();
    let target = root.join("home/config.toml");
    cli::install_launch_links(&[kudo_worker::harness::LaunchLink {
        source: source.clone(),
        target: target.clone(),
    }])
    .unwrap();
    cli::install_launch_links(&[kudo_worker::harness::LaunchLink {
        source,
        target: target.clone(),
    }])
    .unwrap();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "model = 'test'");

    let registry = root.join("installed.json");
    std::thread::scope(|scope| {
        let root = &root;
        let registry = &registry;
        for id in ["one", "two"] {
            scope.spawn(move || {
                let plugin_root = root.join(id);
                let plugin = kudo_worker::harness::LaunchPlugin {
                    id: id.into(),
                    root: plugin_root.clone(),
                    files: vec![kudo_worker::LaunchFile::new(
                        plugin_root.join("kimi.plugin.json"),
                        format!(r#"{{"name":"{id}"}}"#),
                    )],
                };
                cli::install_launch_plugins(&[plugin], registry).unwrap();
            });
        }
    });
    let installed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
    let plugins = installed["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 2, "{installed}");
    assert!(plugins.iter().any(|plugin| plugin["id"] == "one"));
    assert!(plugins.iter().any(|plugin| plugin["id"] == "two"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn kimi_input_translates_console_enter_without_changing_other_keys() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_kudo-shim"))
        .arg("kimi-input")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"text\r\n\x1b[A\x13")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, b"text\x1b[13u\x1b[13u\x1b[A\x13");
}
