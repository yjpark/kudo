//! The arrangement is a value: the same shell, the same panes and data, two
//! layout values, two arrangements — no component changes.
//!
//! implements: ed06836e3296e0ea5caad3e2ca7a972c911743ad1a4f87f149c069132dadac35@115b689ab32e5a9a4ace6a21d50e463c4927e1f41c42513e2b0de4a21166cd97

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use common::{focused, panes, render_themed, render_with};
use kudo_console::layout::{Direction, Layout, Narrow, PaneKind, STACKED, THREE_PANES};
use kudo_console::model::{
    ConsoleData, ContextCard, ContextDetail, NodeView, PaneRef, TerminalView, TraceRow, WorkerView,
};
use kudo_console::selection::{Selection, Tab};
use kudo_console::theme::Theme;

fn sample() -> ConsoleData {
    ConsoleData {
        nodes: vec![
            NodeView {
                id: "yolo".into(),
                links: vec!["stable".into()],
                may_start: true,
            },
            NodeView {
                id: "pc".into(),
                links: vec!["stable".into()],
                may_start: false,
            },
        ],
        contexts: vec![ContextCard {
            id: "c".repeat(64),
            title: "first kudo".into(),
            ready: 1,
            waiting: 0,
            submitted: 2,
            later: 1,
            next_due: Some("2026-10-22T10:00:00Z".into()),
            workers: vec![WorkerView {
                lease: "lease-1".into(),
                last: "pull 1234".into(),
                quiet: false,
                pane: Some(PaneRef {
                    node: "yolo".into(),
                    job: "job-3".into(),
                }),
            }],
        }],
        selected: Some(ContextDetail {
            id: "c".repeat(64),
            title: "first kudo".into(),
            head: "# first kudo\nstate: active\n".into(),
            forked: false,
            trace: vec![TraceRow {
                at: "2026-10-21T00:00:00Z".into(),
                lease: "lease-1".into(),
                seq: 1,
                line: "lease-start".into(),
            }],
        }),
        error: None,
        approvals: vec![],
        approvals_unknown: vec![],
        terminal: Some(TerminalView {
            pane: PaneRef {
                node: "yolo".into(),
                job: "job-3".into(),
            },
            screen: "claude> waiting for you".into(),
        }),
        ..ConsoleData::default()
    }
}

/// The sample's context selected, its worker's pane taken over.
fn selected() -> Selection {
    let mut s = Selection::default();
    s.select_context("c".repeat(64));
    s.pane = Some(PaneRef {
        node: "yolo".into(),
        job: "job-3".into(),
    });
    s
}

fn render(layout: Layout) -> String {
    render_with(layout, sample(), selected())
}

/// The order the panes appear in the rendered page.
fn pane_order(html: &str) -> Vec<&'static str> {
    panes(html)
}

#[test]
fn three_panes_put_where_head_and_trace_side_by_side() {
    let html = render(THREE_PANES);
    assert_eq!(
        pane_order(&html),
        ["where", "head", "trace", "terminal"],
        "{html}"
    );
    assert!(html.contains("claude&#62; waiting for you"), "{html}");
    assert!(
        html.contains("take over"),
        "a worker with a pane offers it: {html}"
    );
    assert!(html.contains("data-direction=\"row\""), "{html}");
    assert!(
        html.contains("first kudo") && html.contains("state: active"),
        "{html}"
    );
    assert!(html.contains("lease-start"), "{html}");
}

#[test]
fn a_second_arrangement_is_only_a_different_value() {
    assert_eq!(STACKED.direction, Direction::Column);
    let kinds = |l: &Layout| {
        let mut k: Vec<PaneKind> = l.panes.iter().map(|p| p.kind).collect();
        k.sort();
        k
    };
    assert_eq!(kinds(&STACKED), kinds(&THREE_PANES), "the same panes");
    let html = render(STACKED);
    assert!(html.contains("data-direction=\"column\""), "{html}");
    assert_ne!(
        pane_order(&html),
        pane_order(&render(THREE_PANES)),
        "a different arrangement"
    );
}

/// A taken-over pane offers the keys a CLI's menus and Kimi's immediate
/// takeover need, each as the bytes a terminal sends.
#[test]
fn a_taken_over_pane_offers_arrow_keys() {
    use kudo_console::shell::KEYS;
    let bytes = |label: &str| KEYS.iter().find(|(l, _)| *l == label).map(|(_, b)| *b);
    assert_eq!(bytes("↑"), Some(&b"\x1b[A"[..]));
    assert_eq!(bytes("↓"), Some(&b"\x1b[B"[..]));
    assert_eq!(bytes("→"), Some(&b"\x1b[C"[..]));
    assert_eq!(bytes("←"), Some(&b"\x1b[D"[..]));
    assert_eq!(bytes("Tab"), Some(&b"\t"[..]));
    assert_eq!(bytes("Enter"), Some(&b"\r"[..]));
    assert_eq!(bytes("Ctrl-S"), Some(&b"\x13"[..]));
    let html = render(THREE_PANES);
    for (label, _) in KEYS {
        assert!(
            html.contains(&format!(">{label}</button>")),
            "{label}: {html}"
        );
    }
}

/// "Start a worker" is offered only on the nodes the console may start one
/// on.
// implements: 2f073c122047d13470407233fd0095dd86408b1ebc66ed6f136c3f3b7027b2c5@2f073c122047d13470407233fd0095dd86408b1ebc66ed6f136c3f3b7027b2c5
#[test]
fn a_worker_is_offered_only_on_the_nodes_allowed() {
    let html = render(THREE_PANES);
    assert!(html.contains("start a worker on yolo"), "{html}");
    assert!(!html.contains("start a worker on pc"), "{html}");
}

/// On a phone one pane fills the screen: the one the selection points at.
/// Which applies is the layout's value, carried to the stylesheet.
// implements: 77d223e031a8cefaf954bd0d4708fc301867286d9d92c547811d5335d7caeb90@77d223e031a8cefaf954bd0d4708fc301867286d9d92c547811d5335d7caeb90
#[test]
fn a_phone_shows_the_pane_the_selection_points_at() {
    assert_eq!(THREE_PANES.narrow, Narrow::Drill);
    let html = render_with(THREE_PANES, sample(), Selection::default());
    assert!(html.contains("data-narrow=\"drill\""), "{html}");
    assert_eq!(focused(&html), Some("where"));
    assert_eq!(panes(&html), ["where"], "nothing selected, only the list");

    let mut s = Selection::default();
    s.select_context("c".repeat(64));
    let html = render_with(THREE_PANES, sample(), s.clone());
    assert_eq!(focused(&html), Some("head"));
    s.tab = Tab::Trace;
    let html = render_with(THREE_PANES, sample(), s);
    assert_eq!(focused(&html), Some("trace"));
    assert!(
        html.contains("data-tab=\"head\"") && html.contains("data-tab=\"trace\""),
        "a phone switches head and trace by tab: {html}"
    );
    let html = render(THREE_PANES);
    assert_eq!(focused(&html), Some("terminal"));
    assert_eq!(
        html.matches("data-focus=\"true\"").count(),
        1,
        "one pane in focus"
    );
}

/// The stylesheet hides every pane but the focused one below the phone
/// width, and nothing above it.
#[test]
fn the_stylesheet_drills_only_on_a_narrow_screen() {
    let css = kudo_console::style::STYLE;
    assert!(css.contains("@media (max-width: 720px)"), "{css}");
    assert!(css.contains("[data-narrow=\"drill\"]"), "{css}");
    assert!(css.contains(":not([data-focus])"), "{css}");
    let html = render(THREE_PANES);
    assert!(html.contains("<style"), "the page carries its stylesheet");
}

/// A back control appears once something is selected, and where you are is
/// named in the top bar.
#[test]
fn the_top_bar_says_where_you_are_and_steps_back() {
    let html = render_with(THREE_PANES, sample(), Selection::default());
    assert!(
        !html.contains("data-back"),
        "nothing to step out of: {html}"
    );
    let html = render(THREE_PANES);
    assert!(html.contains("data-back"), "{html}");
    assert!(html.contains("data-crumb"), "{html}");
    let crumb = &html[html.find("data-crumb").unwrap()..];
    assert!(crumb.contains("first kudo"), "{crumb}");
}

/// The nodes are listed beside the contexts, each a way into its jobs, a
/// peer hub's node marked as such.
// implements: 2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655@2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655
#[test]
fn nodes_are_listed_with_an_icon_by_kind() {
    let mut data = sample();
    data.nodes.push(NodeView {
        id: "edger::yolo".into(),
        links: vec!["stable".into()],
        may_start: false,
    });
    let html = render_with(THREE_PANES, data, Selection::default());
    assert!(html.contains("data-node=\"pc\""), "{html}");
    assert!(html.contains("data-node=\"edger::yolo\""), "{html}");
    assert_ne!(
        kudo_console::shell::node_icon("pc"),
        kudo_console::shell::node_icon("edger::yolo"),
        "a peer hub's node reads apart"
    );
}

/// Light, dark, or the system's: the root says which, and the stylesheet
/// draws both from one set of tokens.
// implements: 2dd989d6ce0f4135c0d8d3ab6e99e065ed03fe3dbf1827d5621d289458cd4351@2dd989d6ce0f4135c0d8d3ab6e99e065ed03fe3dbf1827d5621d289458cd4351
#[test]
fn the_theme_follows_the_system_unless_picked() {
    assert_eq!(Theme::default(), Theme::System);
    assert_eq!(Theme::System.next(), Theme::Dark);
    assert_eq!(Theme::Dark.next(), Theme::Light);
    assert_eq!(
        Theme::Light.next(),
        Theme::System,
        "a way back to the system"
    );
    for t in [Theme::System, Theme::Dark, Theme::Light] {
        assert_eq!(Theme::from_stored(t.stored()), t);
    }
    assert_eq!(Theme::from_stored("purple"), Theme::System);

    let html = render_themed(THREE_PANES, sample(), selected(), Theme::System);
    assert!(!html.contains("class=\"kudo-shell\" data-theme"), "{html}");
    let html = render_themed(THREE_PANES, sample(), selected(), Theme::Dark);
    assert!(
        html.contains("class=\"kudo-shell\" data-theme=\"dark\""),
        "{html}"
    );
    let css = kudo_console::style::STYLE;
    assert!(css.contains("prefers-color-scheme: dark"), "{css}");
    assert!(css.contains("[data-theme=\"dark\"]"), "{css}");
    assert!(css.contains("[data-theme=\"light\"]"), "{css}");
    assert!(css.contains("--bg"), "colours are tokens: {css}");
}

/// While a switch is re-read, the context before must not show as the one
/// selected (nor take its asks); a context that is not here says so.
#[test]
fn a_context_pane_shows_only_the_selected_context() {
    let mut other = Selection::default();
    other.select_context("d".repeat(64));
    let html = render_with(THREE_PANES, sample(), other.clone());
    let markup = common::markup(&html);
    assert!(!markup.contains("state: active"), "{markup}");
    assert!(markup.contains("this context is not here"), "{markup}");

    let mut data = sample();
    data.contexts[0].id = "d".repeat(64);
    let html = render_with(THREE_PANES, data, other);
    let markup = common::markup(&html);
    assert!(!markup.contains("state: active"), "{markup}");
    assert!(markup.contains("loading the context"), "{markup}");
}
