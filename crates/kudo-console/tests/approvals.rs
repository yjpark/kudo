//! The approval queue: every node's jobs waiting for the author's verdict,
//! oldest first, with what is needed to answer — and the four verdicts, plus
//! edit-then-allow, a tap away.
//!
//! implements: kudo:daf63591a53811da5e761c847a528776758b90ee07fc55f8c16ce14789be1f44

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use kudo_console::badge::{Badge, badge};
use kudo_console::layout::THREE_PANES;
use kudo_console::model::{ApprovalView, ConsoleData, McpApprovalView};
use kudo_console::selection::Selection;

fn waiting(node: &str, job: &str, argv: &[&str], waited_s: u64) -> ApprovalView {
    ApprovalView {
        node: node.into(),
        job: job.into(),
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        cwd: "/tmp".into(),
        owner: "/home/yj/agents/edger/kudo".into(),
        waited_s,
        left_s: 1800 - waited_s,
        context: Some("home lab".into()),
    }
}

/// The page with the approvals tray open.
fn render(data: ConsoleData) -> String {
    let html = common::render_with(
        THREE_PANES,
        data,
        Selection {
            tray: true,
            ..Selection::default()
        },
    );
    common::markup(&html).to_owned()
}

/// The page with the tray closed.
fn closed(data: ConsoleData) -> String {
    let html = common::render_with(THREE_PANES, data, Selection::default());
    common::markup(&html).to_owned()
}

/// A waiting job shows its node, argv, where it runs, who asked and the time
/// left, with the verdict buttons.
#[test]
fn a_waiting_job_shows_what_is_needed_to_answer_it() {
    let html = render(ConsoleData {
        approvals: vec![waiting("pc", "j-1", &["df", "-h", "/"], 60)],
        ..ConsoleData::default()
    });
    for text in [
        "approvals (1)",
        "df -h /",
        "on pc in /tmp",
        "/tmp",
        "home lab",
        "29 min left",
        "allow once",
        "allow always",
        "deny once",
        "deny always",
        "edit, then allow",
    ] {
        assert!(html.contains(text), "{text:?} missing from {html}");
    }
}

fn waiting_mcp() -> McpApprovalView {
    McpApprovalView {
        request: "request-1".into(),
        context: "home lab".into(),
        lease: "lease-1".into(),
        tool: "mcp__chat__send_message".into(),
        arguments: r#"{"text":"hello"}"#.into(),
        waited_s: 60,
        left_s: 1740,
    }
}

// implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de@fdae3bbd25e0ab56206b07c5d58d6b105f7a32fa28ba95b81e635252c3295ef2
#[test]
fn an_mcp_write_shows_its_arguments_and_only_single_use_verdicts() {
    let html = render(ConsoleData {
        mcp_approvals: vec![waiting_mcp()],
        ..ConsoleData::default()
    });
    for text in [
        "approvals (1)",
        "mcp__chat__send_message",
        r#"{"text":"hello"}"#,
        "home lab",
        "lease-1",
        "29 min left",
        "allow once",
        "deny once",
    ] {
        assert!(html.contains(text), "{text:?} missing from {html}");
    }
    assert!(!html.contains("allow always"), "{html}");
    assert!(!html.contains("deny always"), "{html}");
    assert!(!html.contains("edit, then allow"), "{html}");

    let badge = closed(ConsoleData {
        mcp_approvals: vec![waiting_mcp()],
        ..ConsoleData::default()
    });
    assert!(badge.contains("1 approval"), "{badge}");
}

/// Nothing waiting: the tray says so in a sentence, and offers no verdict.
#[test]
fn no_waiting_jobs_no_queue() {
    let html = render(ConsoleData::default());
    assert!(html.contains("nothing is waiting for you"), "{html}");
    assert!(!html.contains("allow once"), "{html}");
}

/// The badge tells a trustworthy zero from unknown; a real count wins over
/// trouble.
// implements: 85f973554ab08ddde3cb91944354db7fb14ff55a0da506daea7c4643c8646339@85f973554ab08ddde3cb91944354db7fb14ff55a0da506daea7c4643c8646339
#[test]
fn the_badge_tells_none_from_unknown() {
    assert_eq!(badge(0, 0), Badge::None);
    assert_eq!(badge(2, 0), Badge::Waiting(2));
    assert_eq!(badge(2, 1), Badge::Waiting(2), "a count wins over trouble");
    assert_eq!(badge(0, 1), Badge::Unknown(1));
    assert_eq!(Badge::None.label(), "approvals · none");
    assert_eq!(Badge::Waiting(1).label(), "1 approval");
    assert_eq!(Badge::Waiting(3).label(), "3 approvals");
    assert_eq!(Badge::Unknown(2).label(), "approvals · unknown on 2 nodes");

    let html = closed(ConsoleData::default());
    assert!(html.contains("data-badge=\"none\""), "{html}");
    assert!(!html.contains("data-tray"), "closed: {html}");
    let html = closed(ConsoleData {
        approvals_unknown: vec!["edger".into()],
        ..ConsoleData::default()
    });
    assert!(html.contains("data-badge=\"unknown\""), "{html}");
    let html = closed(ConsoleData {
        approvals: vec![waiting("pc", "j-1", &["df"], 1)],
        ..ConsoleData::default()
    });
    assert!(html.contains("data-badge=\"waiting\""), "{html}");
    assert!(html.contains("1 approval"), "{html}");
}

/// The tray is not a block in the list: closed, no verdict is offered in
/// the page; open, every waiting job is.
#[test]
fn the_queue_lives_in_the_tray() {
    let data = ConsoleData {
        approvals: vec![waiting("pc", "j-1", &["df"], 1)],
        ..ConsoleData::default()
    };
    assert!(!closed(data.clone()).contains("allow once"));
    let html = render(data);
    assert!(html.contains("data-tray"), "{html}");
    assert!(html.contains("allow once"), "{html}");
}

/// The always verdicts ask for a second tap that states the exact rule and
/// the one node it binds; the once verdicts write no rule and go at once.
// implements: 3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5@3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5
#[test]
fn a_standing_verdict_takes_a_second_tap() {
    use kudo_console::confirm::{kill_text, second_tap};
    assert_eq!(second_tap("allow-once", "df -h", "pc"), None);
    assert_eq!(second_tap("deny-once", "df -h", "pc"), None);
    let allow = second_tap("allow-always", "df -h", "pc").expect("asks again");
    assert!(
        allow.contains("df -h") && allow.contains("pc only"),
        "{allow}"
    );
    assert!(allow.contains("always allow"), "{allow}");
    let deny = second_tap("deny-always", "rm -rf /", "edger::yolo").expect("asks again");
    assert!(
        deny.contains("always deny") && deny.contains("edger::yolo only"),
        "{deny}"
    );
    let kill = kill_text("1792500000000-7-1", "pc");
    assert!(
        kill.contains("1792500000000-7-1") && kill.contains("pc"),
        "{kill}"
    );
    assert_eq!(kudo_console::confirm::CONFIRM_MS, 10_000);
}

#[cfg(feature = "server")]
mod server_side {
    use std::collections::HashMap;

    use kudo_console::views;
    use kudo_context::{ContextStore, TraceEvent};
    use moco_job::Scope;
    use moco_job::wire::{PendingJob, WireVerdict};

    const T0: i64 = 1_792_500_000;

    fn pending(id: &str, waited_ms: u64) -> PendingJob {
        PendingJob {
            id: id.into(),
            argv: vec!["df".into(), "-h".into()],
            cwd: "/tmp".into(),
            scope: Scope::System,
            waited_ms,
            left_ms: 1_800_000 - waited_ms,
        }
    }

    /// Every node's waiting jobs, oldest first, each named by the context
    /// whose worker asked (when one traced it).
    #[test]
    fn approvals_are_oldest_first_and_name_the_context_that_asked() {
        let mut asked = HashMap::new();
        asked.insert("j-old".to_owned(), "home lab".to_owned());
        let views = views::approvals(
            vec![
                ("yolo".into(), pending("j-new", 1_000)),
                ("pc".into(), pending("j-old", 90_000)),
            ],
            &asked,
        );
        let order: Vec<&str> = views.iter().map(|v| v.job.as_str()).collect();
        assert_eq!(order, vec!["j-old", "j-new"]);
        // Rows never move under a finger: equal waits keep one order.
        let tied = |a: &str, b: &str| {
            views::approvals(
                vec![
                    (a.into(), pending("j-x", 5_000)),
                    (b.into(), pending("j-x", 5_000)),
                ],
                &HashMap::new(),
            )
            .into_iter()
            .map(|v| v.node)
            .collect::<Vec<_>>()
        };
        assert_eq!(tied("pc", "edger"), tied("edger", "pc"));
        assert_eq!(views[0].node, "pc");
        assert_eq!(views[0].context.as_deref(), Some("home lab"));
        assert_eq!(views[0].waited_s, 90);
        assert_eq!(views[1].context, None);
    }

    /// Which context's worker started a job, from the job-start events.
    #[test]
    fn the_context_that_asked_is_read_from_the_trace() {
        let dir = std::env::temp_dir().join(format!("kudo-console-asked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store
            .create_context("home lab", "# home lab\n", T0)
            .unwrap();
        store
            .record_trace(
                &ctx,
                "lease-1",
                &TraceEvent::JobStart {
                    node: "pc".into(),
                    job: "j-1".into(),
                    waiting: true,
                    argv: "df -h".into(),
                },
                T0,
            )
            .unwrap();
        let asked = views::job_contexts(&store).unwrap();
        let name = asked.get("j-1").expect("the job is named by its context");
        assert!(name.starts_with("home lab "), "{name}");
        assert!(
            name.ends_with(&ctx.0[..12]),
            "titles need not be unique: {name}"
        );
    }

    // implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de@fdae3bbd25e0ab56206b07c5d58d6b105f7a32fa28ba95b81e635252c3295ef2
    #[test]
    fn mcp_requests_become_approval_views_with_time_left() {
        let dir = std::env::temp_dir().join(format!("kudo-console-mcp-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ContextStore::init(&dir).unwrap();
        let ctx = store.create_context("home lab", "# home lab\n", T0).unwrap();
        let request = store
            .request_mcp(&ctx, "lease-1", "mcp__chat__send_message", "{}", T0)
            .unwrap();
        let pending = store.pending_mcp(T0 + 60).unwrap();
        let views = views::mcp_approvals(pending, T0 + 60);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].request, request.id);
        assert_eq!(views[0].context, ctx.0);
        assert_eq!(views[0].lease, "lease-1");
        assert_eq!(views[0].waited_s, 60);
        assert_eq!(views[0].left_s, 1740);
    }

    /// The verdict names the buttons send.
    #[test]
    fn verdict_names_map_to_the_four_verdicts() {
        assert_eq!(
            views::verdict_from("allow-once"),
            Some(WireVerdict::AllowOnce)
        );
        assert_eq!(
            views::verdict_from("allow-always"),
            Some(WireVerdict::AllowAlways)
        );
        assert_eq!(
            views::verdict_from("deny-once"),
            Some(WireVerdict::DenyOnce)
        );
        assert_eq!(
            views::verdict_from("deny-always"),
            Some(WireVerdict::DenyAlways)
        );
        assert_eq!(views::verdict_from("maybe"), None);
    }

    /// Every button sends a verdict the server reads.
    #[test]
    fn every_button_sends_a_known_verdict() {
        for (label, verdict) in kudo_console::shell::VERDICTS {
            assert!(
                views::verdict_from(verdict).is_some(),
                "{label} sends {verdict}"
            );
        }
        assert_eq!(kudo_console::shell::VERDICTS.len(), 4);
    }
}

/// An argv reads and edits as one shell-quoted line: an element with a space
/// stays one element, and an unchanged edit is the same argv.
#[test]
fn an_argv_round_trips_through_its_text() {
    use kudo_console::model::{argv_parse, argv_text};
    let argv: Vec<String> = ["sh", "-c", "rm -rf /tmp/x", ""]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    let text = argv_text(&argv);
    assert_ne!(text, "sh -c rm -rf /tmp/x ", "quoted, not joined: {text}");
    assert_eq!(argv_parse(&text), Some(argv));
    assert_eq!(argv_parse("   "), None, "an empty edit is refused");
    assert_eq!(argv_parse("echo 'unclosed"), None);
}

/// A node whose queue could not be read is named in the tray, and the tray
/// does not claim nothing is waiting.
#[test]
fn an_unreadable_queue_is_said() {
    let html = render(ConsoleData {
        approvals_unknown: vec!["edger".into()],
        ..ConsoleData::default()
    });
    assert!(html.contains("not answering: edger"), "{html}");
    assert!(!html.contains("nothing is waiting for you"), "{html}");
}

/// Allow and deny sit on rows of their own: a thumb aimed at "allow always"
/// does not land on "deny once" (found live, 22 Oct).
#[test]
fn allow_and_deny_are_on_separate_rows() {
    let html = render(ConsoleData {
        approvals: vec![waiting("pc", "j-1", &["df"], 1)],
        ..ConsoleData::default()
    });
    let allow = html.find(">allow always<").expect("allow always");
    let deny = html.find(">deny once<").expect("deny once");
    let between = &html[allow..deny];
    assert!(
        between.contains("class=\"row\""),
        "a new row starts between: {between}"
    );
}
