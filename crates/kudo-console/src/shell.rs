//! The shell: places panes by the layout value and renders each by its kind.
//! No pane fetches anything; each draws from the data it is handed. Around
//! the panes: the top bar (back, where you are, the approvals badge, the
//! theme) and the approvals tray.
//!
//! implements: ed06836e3296e0ea5caad3e2ca7a972c911743ad1a4f87f149c069132dadac35@115b689ab32e5a9a4ace6a21d50e463c4927e1f41c42513e2b0de4a21166cd97
//! implements: 77d223e031a8cefaf954bd0d4708fc301867286d9d92c547811d5335d7caeb90@77d223e031a8cefaf954bd0d4708fc301867286d9d92c547811d5335d7caeb90
//! implements: 85f973554ab08ddde3cb91944354db7fb14ff55a0da506daea7c4643c8646339@85f973554ab08ddde3cb91944354db7fb14ff55a0da506daea7c4643c8646339

use dioxus::prelude::*;

use crate::badge::badge;
use crate::confirm::{CONFIRM_MS, kill_text, second_tap, verdict_hint};
use crate::layout::{Direction, Layout, Narrow, PaneKind};
use crate::model::ConsoleData;
use crate::selection::{Notice, Outcome, Selection, Tab};
use crate::theme::Theme;

/// The panes of `layout`, drawn from `data`, under the page's selection.
#[component]
pub fn Shell(layout: Layout, data: ConsoleData) -> Element {
    let selection = try_consume_context::<Signal<Selection>>();
    let sel = selection.map(|s| s.read().clone()).unwrap_or_default();
    let theme = try_consume_context::<Signal<Theme>>()
        .map(|t| t())
        .unwrap_or_default();
    let direction = match layout.direction {
        Direction::Row => "row",
        Direction::Column => "column",
    };
    let narrow = match layout.narrow {
        Narrow::Drill => "drill",
        Narrow::Same => "same",
    };
    let focus = sel.focus();
    let notice = try_consume_context::<Signal<Notice>>().and_then(|n| n.read().0.clone());
    // One armed button at a time: a tap anywhere else disarms it.
    let mut armed = use_context_provider(|| Signal::new(Armed::default()));
    rsx! {
        div {
            class: "kudo-shell",
            "data-theme": theme.attr(),
            "data-narrow": narrow,
            onclick: move |_| {
                if armed.peek().id.is_some() {
                    armed.write().id = None;
                }
            },
            style { dangerous_inner_html: crate::style::STYLE }
            TopBar { data: data.clone(), selection: sel.clone() }
            if let Some(error) = data.error.clone() {
                div { class: "kudo-band", "{error}" }
            }
            if let Some(said) = notice {
                div { class: "kudo-band",
                    "{said}"
                    button {
                        onclick: move |_| {
                            if let Some(mut n) = try_consume_context::<Signal<Notice>>() {
                                n.set(Notice(None));
                            }
                        },
                        "×"
                    }
                }
            }
            div { class: "kudo-panes", "data-direction": direction,
                for pane in layout.panes.iter().filter(|p| sel.shows(p.kind)) {
                    div {
                        key: "{pane.kind:?}",
                        "data-pane": pane_name(pane.kind),
                        "data-focus": (pane.kind == focus).then_some("true"),
                        style: "flex:{pane.grow};",
                        {render(pane.kind, &data, &sel)}
                    }
                }
            }
            if sel.tray {
                Tray { data: data.clone() }
            }
        }
    }
}

fn pane_name(kind: PaneKind) -> &'static str {
    match kind {
        PaneKind::Where => "where",
        PaneKind::Head => "head",
        PaneKind::Trace => "trace",
        PaneKind::Jobs => "jobs",
        PaneKind::Job => "job",
        PaneKind::Terminal => "terminal",
    }
}

/// The renderer for a pane's kind.
fn render(kind: PaneKind, data: &ConsoleData, sel: &Selection) -> Element {
    match kind {
        PaneKind::Where => rsx! { WherePane { data: data.clone(), selection: sel.clone() } },
        PaneKind::Head => rsx! { HeadPane { data: data.clone(), selection: sel.clone() } },
        PaneKind::Trace => rsx! { TracePane { data: data.clone(), selection: sel.clone() } },
        PaneKind::Jobs => rsx! { JobsPane { data: data.clone(), selection: sel.clone() } },
        PaneKind::Job => rsx! { JobPane { data: data.clone(), selection: sel.clone() } },
        PaneKind::Terminal => rsx! { TerminalPane { data: data.clone() } },
    }
}

fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

/// Change the page's selection (nothing, when rendered without a page). An
/// armed button is disarmed: the second tap belongs to what was shown.
fn select(change: impl FnOnce(&mut Selection)) {
    disarm();
    if let Some(mut selection) = try_consume_context::<Signal<Selection>>() {
        change(&mut selection.write());
    }
}

/// The button waiting for its second tap, by id, and how many armings there
/// have been — so a timer from an earlier arming never cuts a later one short.
// implements: 3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5@3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Armed {
    pub id: Option<String>,
    pub armings: u64,
}

/// Arm the button `id`; it disarms itself after [`CONFIRM_MS`] unless armed
/// again since.
fn arm(id: String) {
    let Some(mut armed) = try_consume_context::<Signal<Armed>>() else {
        return;
    };
    let this = {
        let mut a = armed.write();
        a.armings += 1;
        a.id = Some(id);
        a.armings
    };
    spawn(async move {
        crate::app::pause(CONFIRM_MS).await;
        if armed.peek().armings == this && armed.peek().id.is_some() {
            armed.write().id = None;
        }
    });
}

/// Whether the button `id` is armed.
fn is_armed(id: &str) -> bool {
    try_consume_context::<Signal<Armed>>().is_some_and(|a| a.read().id.as_deref() == Some(id))
}

fn disarm() {
    if let Some(mut armed) = try_consume_context::<Signal<Armed>>()
        && armed.peek().id.is_some()
    {
        armed.write().id = None;
    }
}

/// Step out one level. Where the page pushed history for the level, it is
/// the browser's back — so the phone's own back and this one agree.
pub fn step_back() {
    if crate::app::history_back() {
        return;
    }
    select(Selection::back);
}

/// A node's icon by kind: a peer hub's node reads apart from this hub's.
pub fn node_icon(id: &str) -> &'static str {
    if id.contains("::") { "⇄" } else { "▣" }
}

/// Where you are, in words.
fn crumb(data: &ConsoleData, sel: &Selection) -> String {
    let mut parts = vec!["kudo".to_owned()];
    if let Some(c) = &sel.context {
        let title = data
            .selected
            .as_ref()
            .filter(|d| &d.id == c)
            .map(|d| d.title.clone())
            .or_else(|| {
                data.contexts
                    .iter()
                    .find(|k| &k.id == c)
                    .map(|k| k.title.clone())
            })
            .unwrap_or_else(|| short(c).to_owned());
        parts.push(title);
        if sel.tab == Tab::Trace {
            parts.push("trace".to_owned());
        }
    }
    if let Some(n) = &sel.node {
        parts.push(n.clone());
    }
    if let Some(j) = &sel.job {
        parts.push(format!("job {j}"));
    }
    if let Some(p) = &sel.pane {
        parts.push(format!("pane {} on {}", p.job, p.node));
    }
    parts.join(" › ")
}

#[component]
fn TopBar(data: ConsoleData, selection: Selection) -> Element {
    let state = badge(
        data.approvals.len() + data.mcp_approvals.len(),
        data.approvals_unknown.len(),
    );
    let theme = try_consume_context::<Signal<Theme>>();
    let current = theme.map(|t| t()).unwrap_or_default();
    rsx! {
        div { class: "kudo-top",
            if selection.depth() > 0 {
                button { "data-back": "true", onclick: move |_| step_back(), "‹ back" }
            }
            span { class: "crumb", "data-crumb": "true", "{crumb(&data, &selection)}" }
            button {
                class: "badge",
                "data-badge": state.state(),
                title: state.hint(),
                onclick: move |_| select(|s| s.tray = true),
                "{state.label()}"
            }
            button {
                class: "theme",
                title: current.label(),
                onclick: move |_| {
                    if let Some(mut theme) = theme {
                        let next = theme().next();
                        theme.set(next);
                    }
                },
                "◐"
            }
        }
    }
}

#[component]
fn WherePane(data: ConsoleData, selection: Selection) -> Element {
    rsx! {
        h3 { "contexts" }
        if data.contexts.is_empty() {
            p { class: "muted", "no contexts yet" }
        }
        for card in data.contexts.iter() {
            div {
                key: "{card.id}",
                class: "item",
                "data-selected": (selection.context.as_ref() == Some(&card.id)).then_some("true"),
                onclick: {
                    let id = card.id.clone();
                    move |_| select(|s| s.select_context(id.clone()))
                },
                div { span { class: "title", "{card.title}" } span { class: "muted", " {short(&card.id)}" } }
                div { class: "muted",
                    "{card.ready} ready · {card.submitted} submitted"
                    if card.waiting > 0 {
                        " · {card.waiting} waiting"
                    }
                    if let Some(next) = &card.next_due {
                        " · {card.later} later (next {next})"
                    }
                }
                div { class: "workers",
                    for worker in card.workers.iter() {
                        div { key: "{worker.lease}", class: "row",
                            span {
                                "worker {short(&worker.lease)}: {worker.last}"
                                if worker.quiet {
                                    " (quiet — likely gone)"
                                }
                            }
                            if let Some(pane) = worker.pane.clone() {
                                button {
                                    onclick: move |e: MouseEvent| {
                                        e.stop_propagation();
                                        let pane = pane.clone();
                                        select(|s| s.pane = Some(pane));
                                    },
                                    "take over"
                                }
                            }
                        }
                    }
                }
                div { class: "row",
                    for node in data.nodes.iter().filter(|n| n.may_start) {
                        button {
                            key: "start-{node.id}",
                            onclick: {
                                let context = card.id.clone();
                                let node = node.id.clone();
                                move |e: MouseEvent| {
                                    e.stop_propagation();
                                    let (context, node) = (context.clone(), node.clone());
                                    async move {
                                        match crate::api::start_worker(context, node).await {
                                            Ok(pane) => select(|s| s.pane = Some(pane)),
                                            Err(e) => say(e.to_string()),
                                        }
                                    }
                                }
                            },
                            "start a worker on {node.id}"
                        }
                    }
                }
            }
        }
        h3 { "nodes" }
        if data.nodes.is_empty() {
            p { class: "muted", "no nodes (is the hub up?)" }
        }
        for node in data.nodes.iter() {
            div {
                key: "{node.id}",
                class: "item",
                "data-node": "{node.id}",
                "data-selected": (selection.node.as_ref() == Some(&node.id)).then_some("true"),
                onclick: {
                    let id = node.id.clone();
                    move |_| select(|s| s.select_node(id.clone()))
                },
                "{node_icon(&node.id)} {node.id} "
                span { class: "muted", "{node.links.join(\", \")}" }
            }
        }
    }
}

/// Head or trace, on a phone, where only one of them shows.
#[component]
fn Tabs(tab: Tab) -> Element {
    rsx! {
        div { class: "tabs narrow-only",
            button {
                "data-tab": "head",
                "data-on": (tab == Tab::Head).then_some("true"),
                onclick: move |_| select(|s| s.tab = Tab::Head),
                "head"
            }
            button {
                "data-tab": "trace",
                "data-on": (tab == Tab::Trace).then_some("true"),
                onclick: move |_| select(|s| s.tab = Tab::Trace),
                "trace"
            }
        }
    }
}

/// The selected context's detail, when the data is for it: a re-read after a
/// switch still holds the one before, which must not show (or take an ask)
/// as this one.
fn detail_for<'d>(
    data: &'d ConsoleData,
    sel: &Selection,
) -> Option<&'d crate::model::ContextDetail> {
    let context = sel.context.as_deref()?;
    data.selected.as_ref().filter(|d| d.id.starts_with(context))
}

/// What a context pane says while its detail is not here: loading, or gone.
#[component]
fn NoDetail(data: ConsoleData, selection: Selection) -> Element {
    let context = selection.context.clone().unwrap_or_default();
    let listed = data.contexts.iter().any(|c| c.id.starts_with(&context));
    rsx! {
        Tabs { tab: selection.tab }
        if listed {
            p { class: "muted", "loading the context…" }
        } else {
            p { "this context is not here (gone, or a stale link)" }
            button { onclick: move |_| select(|s| *s = Selection::default()), "back to the list" }
        }
    }
}

#[component]
fn HeadPane(data: ConsoleData, selection: Selection) -> Element {
    let mut ask = use_signal(String::new);
    let tab = selection.tab;
    match detail_for(&data, &selection) {
        None => rsx! { NoDetail { data: data.clone(), selection: selection.clone() } },
        Some(detail) => {
            let context = detail.id.clone();
            rsx! {
                Tabs { tab }
                h3 { "{detail.title}" }
                form {
                    class: "row",
                    onsubmit: move |e: FormEvent| {
                        e.prevent_default();
                        let context = context.clone();
                        let text = ask();
                        async move {
                            if text.trim().is_empty() {
                                return;
                            }
                            match crate::api::enqueue(context, text).await {
                                Ok(_) => ask.set(String::new()),
                                Err(e) => say(e.to_string()),
                            }
                        }
                    },
                    input {
                        placeholder: "enqueue an ask under this context",
                        value: "{ask}",
                        oninput: move |e| ask.set(e.value()),
                    }
                    button { r#type: "submit", "enqueue" }
                }
                if detail.forked {
                    p { class: "muted", "the head is forked: the next submit folds the tips together" }
                }
                pre { "{detail.head}" }
            }
        }
    }
}

#[component]
fn TracePane(data: ConsoleData, selection: Selection) -> Element {
    let tab = selection.tab;
    match detail_for(&data, &selection) {
        None => rsx! { NoDetail { data: data.clone(), selection: selection.clone() } },
        Some(detail) => rsx! {
            Tabs { tab }
            h3 { "trace" }
            for row in detail.trace.iter() {
                div { key: "{row.lease}-{row.seq}", class: "trace-row",
                    span { class: "muted", "{row.at}  {short(&row.lease)} #{row.seq}  " }
                    "{row.line}"
                }
            }
        },
    }
}

/// A node's jobs: waiting, running, recently ended.
// implements: 2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655@2d5dba56c923d44ea12676229e18088a19394679ab5dfcbbb25b10b08b438655
#[component]
fn JobsPane(data: ConsoleData, selection: Selection) -> Element {
    let node = selection.node.clone().unwrap_or_default();
    let Some(jobs) = data.jobs.clone().filter(|j| j.node == node) else {
        return rsx! {
            h3 { "{node_icon(&node)} {node}" }
            p { class: "muted", "reading the jobs on {node}…" }
        };
    };
    rsx! {
        h3 { "{node_icon(&node)} {node} · jobs" }
        if let Some(error) = &jobs.error {
            p { class: "kudo-band", "the jobs on {node} could not be read: {error}" }
        } else if jobs.jobs.is_empty() {
            p { class: "muted", "no jobs on {node}" }
        }
        for job in jobs.jobs.iter() {
            div {
                key: "{job.id}",
                class: "item job",
                "data-job": "{job.id}",
                "data-tone": "{job.tone}",
                "data-selected": (selection.job.as_ref() == Some(&job.id)).then_some("true"),
                onclick: {
                    let id = job.id.clone();
                    move |_| select(|s| s.select_job(id.clone()))
                },
                div {
                    span { class: "state", "{job.state}" }
                    " {job.argv}"
                }
                div { class: "muted", "{job.cwd} · {job.verdict} · {job.phase}" }
            }
        }
    }
}

/// One job's output, followed while it runs, and a kill after a second tap.
#[component]
fn JobPane(data: ConsoleData, selection: Selection) -> Element {
    let node = selection.node.clone().unwrap_or_default();
    let job = selection.job.clone().unwrap_or_default();
    let listed = data
        .jobs
        .as_ref()
        .and_then(|j| j.jobs.iter().find(|v| v.id == job).cloned());
    let output = data
        .output
        .clone()
        .filter(|o| o.node == node && o.job == job);
    let live = output
        .as_ref()
        .map(|o| o.live)
        .or(listed.as_ref().map(|l| l.live))
        .unwrap_or(false);
    rsx! {
        h3 { "job {job} on {node}" }
        if let Some(listed) = &listed {
            div { class: "row", "data-tone": "{listed.tone}",
                span { class: "state", "{listed.state}" }
                span { "{listed.argv}" }
            }
            div { class: "muted", "{listed.cwd} · {listed.verdict} · {listed.phase}" }
        }
        if live {
            KillButton { key: "{node}/{job}", node: node.clone(), job: job.clone() }
        }
        match output {
            None => rsx! { p { class: "muted", "reading its output…" } },
            Some(out) => rsx! {
                if out.skipped > 0 {
                    p { class: "muted", "{out.skipped} bytes of earlier output were dropped by the node" }
                }
                // The listing already says the state; the output's own says
                // it when the job is not listed (gone, unreadable).
                if listed.is_none() || out.tone == "bad" && !out.live && out.text.is_empty() {
                    div { class: "row", "data-tone": "{out.tone}",
                        span { class: "state", "{out.state}" }
                    }
                }
                div { class: "follow", "data-follow": "true",
                    pre { "{out.text}" }
                }
            },
        }
    }
}

/// Kill a job: the first tap says what the second will do; what the kill
/// led to is said beside it.
// implements: 3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5@3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5
#[component]
fn KillButton(node: String, job: String) -> Element {
    let mut busy = use_signal(|| false);
    let mut result = use_signal(|| None::<(bool, String)>);
    let id = format!("kill:{node}/{job}");
    let armed = is_armed(&id);
    let text = kill_text(&job, &node);
    rsx! {
        div { class: "row",
            button {
                class: if armed { "armed" } else { "bad" },
                disabled: busy(),
                onclick: move |e: MouseEvent| {
                    e.stop_propagation();
                    if !is_armed(&id) {
                        arm(id.clone());
                        return;
                    }
                    disarm();
                    busy.set(true);
                    let (node, job) = (node.clone(), job.clone());
                    spawn(async move {
                        result.set(Some(match crate::api::kill_job(node, job).await {
                            Ok(done) => (false, done),
                            Err(e) => (true, e.to_string()),
                        }));
                        busy.set(false);
                    });
                },
                "kill"
            }
        }
        if armed {
            p { class: "confirm", "{text}" }
        }
        if let Some((failed, said)) = result() {
            p { class: if failed { "result failed" } else { "result" }, "{said}" }
        }
    }
}

/// The approvals tray: every waiting job, the nodes that could not be read,
/// the last outcome. It stays open after a verdict; Esc, × or back close it.
#[component]
fn Tray(data: ConsoleData) -> Element {
    let outcome = try_consume_context::<Signal<Outcome>>()
        .map(|o| o.read().clone())
        .unwrap_or_default();
    rsx! {
        div { class: "tray-dim", onclick: move |_| step_back() }
        div {
            class: "tray",
            "data-tray": "true",
            div { class: "row",
                h3 { style: "flex:1;", "approvals ({data.approvals.len() + data.mcp_approvals.len()})" }
                button { onclick: move |_| step_back(), "×" }
            }
            if let Some(text) = outcome.text {
                p { class: if outcome.failed { "outcome failed" } else { "outcome" }, "{text}" }
            }
            if !data.approvals_unknown.is_empty() {
                p { class: "unknown",
                    "not answering: {data.approvals_unknown.join(\", \")} (retrying)"
                }
            }
            if data.approvals.is_empty() && data.mcp_approvals.is_empty() && data.approvals_unknown.is_empty() {
                p { class: "muted", "nothing is waiting for you" }
            }
            for approval in data.approvals.iter() {
                ApprovalCard { key: "{approval.node}-{approval.job}", approval: approval.clone() }
            }
            for approval in data.mcp_approvals.iter() {
                McpApprovalCard { key: "{approval.request}", approval: approval.clone() }
            }
        }
    }
}

/// One external MCP call waiting for a single-use verdict.
// implements: 53544d31b7dbadd28d6f2b3e9de57b79cb6cdf089069f2062418879f4d3621de@fdae3bbd25e0ab56206b07c5d58d6b105f7a32fa28ba95b81e635252c3295ef2
#[component]
fn McpApprovalCard(approval: crate::model::McpApprovalView) -> Element {
    let mut busy = use_signal(|| false);
    let left = approval.left_s.div_ceil(60);
    let send = move |verdict: &'static str| {
        let request = approval.request.clone();
        async move {
            if busy() {
                return;
            }
            busy.set(true);
            said(None);
            match crate::api::decide_mcp(request, verdict.to_owned()).await {
                Ok(done) => said(Some((false, done))),
                Err(e) => said(Some((true, format!("not decided: {e}")))),
            }
            busy.set(false);
        }
    };
    rsx! {
        div { class: "card",
            div { class: "argv", "{approval.tool}" }
            pre { class: "argv", "{approval.arguments}" }
            div { "MCP call · {left} min left" }
            div { class: "muted", "asked by {approval.context} · lease {approval.lease}" }
            div { class: "row", style: "margin-top:6px;",
                button {
                    disabled: busy(),
                    onclick: {
                        let send = send.clone();
                        move |e: MouseEvent| {
                            e.stop_propagation();
                            spawn(send("allow-once"));
                        }
                    },
                    "allow once"
                }
            }
            div { class: "row", style: "margin-top:18px;",
                button {
                    class: "bad",
                    disabled: busy(),
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        spawn(send("deny-once"));
                    },
                    "deny once"
                }
            }
        }
    }
}

/// The verdicts a waiting job offers: the button's label, and the verdict it
/// sends.
pub const VERDICTS: &[(&str, &str)] = &[
    ("allow once", "allow-once"),
    ("allow always", "allow-always"),
    ("deny once", "deny-once"),
    ("deny always", "deny-always"),
];

/// One job waiting for the author's verdict: what it would run, where, who
/// asked, how long is left — and the verdicts, plus edit-then-allow. The
/// always verdicts take a second tap.
// implements: kudo:daf63591a53811da5e761c847a528776758b90ee07fc55f8c16ce14789be1f44
// implements: 3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5@3cebbf2e8034b89c65bb3dda9ae2be880cf6c5ba2cd928b559e7fb6dd03579f5
#[component]
fn ApprovalCard(approval: crate::model::ApprovalView) -> Element {
    let mut editing = use_signal(|| None::<String>);
    // One verdict at a time: the buttons wait while one is on its way.
    let mut busy = use_signal(|| false);
    let argv = crate::model::argv_text(&approval.argv);
    let key = format!("{}/{}", approval.node, approval.job);
    let left = approval.left_s.div_ceil(60);
    let (node, job) = (approval.node.clone(), approval.job.clone());
    let send = move |verdict: &'static str, edited: Option<Vec<String>>| {
        let (node, job) = (node.clone(), job.clone());
        async move {
            if busy() {
                return;
            }
            busy.set(true);
            said(None);
            match crate::api::decide(node, job, verdict.to_owned(), edited).await {
                Ok(done) => said(Some((false, done))),
                Err(e) => said(Some((true, format!("not decided: {e}")))),
            }
            busy.set(false);
        }
    };
    rsx! {
        div { class: "card",
            div { class: "argv", "{argv}" }
            div { "on {approval.node} in {approval.cwd} · {left} min left" }
            div { class: "muted",
                if let Some(context) = &approval.context {
                    "asked by {context} · "
                }
                "owner {approval.owner}"
            }
            // Allow and deny on rows of their own, a gap between: on a phone a
            // thumb aimed at one must not land on the other.
            for allowing in [true, false] {
              div { class: "row", style: if allowing { "margin-top:6px;" } else { "margin-top:18px;" },
                for (label, verdict) in VERDICTS.iter().copied().filter(|(_, v)| v.starts_with("allow") == allowing) {
                    button {
                        key: "{verdict}",
                        disabled: busy(),
                        class: if is_armed(&format!("{verdict}:{key}")) { "armed" } else if verdict.starts_with("deny") { "bad" } else { "" },
                        title: verdict_hint(verdict, &argv, &approval.node),
                        onclick: {
                            let send = send.clone();
                            let confirm = second_tap(verdict, &argv, &approval.node);
                            let id = format!("{verdict}:{key}");
                            move |e: MouseEvent| {
                                e.stop_propagation();
                                if confirm.is_some() && !is_armed(&id) {
                                    arm(id.clone());
                                    return;
                                }
                                disarm();
                                spawn(send(verdict, None));
                            }
                        },
                        // The armed button keeps its label (and so its size):
                        // nothing moves under the thumb coming back for the
                        // second tap. What it will do is said below the rows.
                        "{label}"
                    }
                }
                if allowing {
                    button {
                        onclick: {
                            let argv = argv.clone();
                            move |_| editing.set(Some(argv.clone()))
                        },
                        "edit, then allow"
                    }
                }
              }
            }
            for (_, verdict) in VERDICTS.iter().copied().filter(|(_, v)| is_armed(&format!("{v}:{key}"))) {
                p { class: "confirm", "{second_tap(verdict, &argv, &approval.node).unwrap_or_default()}" }
            }
            if let Some(text) = editing() {
                form {
                    class: "row",
                    onsubmit: {
                        let send = send.clone();
                        let text = text.clone();
                        move |e: FormEvent| {
                            e.prevent_default();
                            let send = send.clone();
                            let edited = crate::model::argv_parse(&text);
                            if edited.is_some() {
                                editing.set(None);
                            }
                            async move {
                                match edited {
                                    Some(argv) => send("allow-once", Some(argv)).await,
                                    None => say(
                                        "the edited command is empty or does not parse \
                                         (unclosed quote?)"
                                            .to_owned(),
                                    ),
                                }
                            }
                        }
                    },
                    input {
                        value: "{text}",
                        oninput: move |e| editing.set(Some(e.value())),
                    }
                    button { r#type: "submit", disabled: busy(), "allow this" }
                }
            }
        }
    }
}

/// Say what failed, above the panes (when the page provides the notice).
fn say(text: String) {
    if let Some(mut notice) = try_consume_context::<Signal<Notice>>() {
        notice.set(Notice(Some(text)));
    }
}

/// Say what a verdict led to (`(failed, text)`), in the tray, or clear it
/// (when the page provides it).
fn said(what: Option<(bool, String)>) {
    if let Some(mut outcome) = try_consume_context::<Signal<Outcome>>() {
        outcome.set(match what {
            Some((failed, text)) => Outcome {
                text: Some(text),
                failed,
            },
            None => Outcome::default(),
        });
    }
}

/// Type `bytes` into a pane, in pieces a node takes whole; whether it all went.
async fn type_all(pane: crate::model::PaneRef, bytes: Vec<u8>) -> bool {
    for piece in bytes.chunks(4096) {
        if let Err(e) =
            crate::api::type_into(pane.node.clone(), pane.job.clone(), piece.to_vec()).await
        {
            say(format!("not typed: {e}"));
            return false;
        }
    }
    true
}

/// The keys a taken-over pane offers, as the bytes a terminal sends: what a
/// CLI's menus and prompts need beyond a typed line.
pub const KEYS: &[(&str, &[u8])] = &[
    ("↑", b"\x1b[A"),
    ("↓", b"\x1b[B"),
    ("←", b"\x1b[D"),
    ("→", b"\x1b[C"),
    ("Tab", b"\t"),
    ("Enter", b"\r"),
    ("Esc", b"\x1b"),
    ("Ctrl-C", b"\x03"),
    ("Ctrl-S", b"\x13"),
];

/// The bytes for a key pressed in the empty line field: the arrows and
/// Escape go straight to the pane (a menu answers them); anything else types
/// into the field.
fn key_bytes(key: &Key) -> Option<&'static [u8]> {
    let label = match key {
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::Escape => "Esc",
        _ => return None,
    };
    KEYS.iter().find(|(l, _)| *l == label).map(|(_, b)| *b)
}

/// One send into a pane, queued: the bytes, and the typed line they carry
/// (put back in the field if refused).
struct Typed {
    pane: crate::model::PaneRef,
    bytes: Vec<u8>,
    line: Option<String>,
}

/// A worker's pane, taken over: its screen, and keys to type into it.
#[component]
fn TerminalPane(data: ConsoleData) -> Element {
    let Some(terminal) = data.terminal.clone() else {
        return rsx! { p { class: "muted", "reading the pane…" } };
    };
    let mut typed = use_signal(String::new);
    // One queue for everything typed into the pane: each send waits for the
    // one before, so quick keys (↓ ↓ Enter) reach the pane in order.
    let sends = use_coroutine(move |mut rx: UnboundedReceiver<Typed>| async move {
        use futures_util::StreamExt;
        while let Some(send) = rx.next().await {
            let went = type_all(send.pane, send.bytes).await;
            // A refused line is not lost: it goes back in the field.
            if let (false, Some(line)) = (went, send.line) {
                typed.set(line);
            }
        }
    });
    let pane = terminal.pane.clone();
    let key = move |bytes: &[u8]| {
        sends.send(Typed {
            pane: pane.clone(),
            bytes: bytes.to_vec(),
            line: None,
        });
    };
    let direct = key.clone();
    let line_pane = terminal.pane.clone();
    rsx! {
        div { class: "row",
            h3 { style: "flex:1;", "pane {terminal.pane.job} on {terminal.pane.node}" }
            button { onclick: move |_| step_back(), "close" }
        }
        pre { class: "term", "{terminal.screen}" }
        form {
            class: "row",
            onsubmit: move |e: FormEvent| {
                e.prevent_default();
                let line = typed();
                let mut bytes = line.clone().into_bytes();
                bytes.push(b'\r');
                typed.set(String::new());
                sends.send(Typed {
                    pane: line_pane.clone(),
                    bytes,
                    line: Some(line),
                });
            },
            input {
                placeholder: "type into the pane (Enter sends)",
                value: "{typed}",
                oninput: move |e| typed.set(e.value()),
                onkeydown: move |e: KeyboardEvent| {
                    let bytes = typed().is_empty().then(|| key_bytes(&e.key())).flatten();
                    if let Some(bytes) = bytes {
                        e.prevent_default();
                        direct(bytes);
                    }
                },
            }
            button { r#type: "submit", "send" }
        }
        div { class: "row",
            for (label, bytes) in KEYS.iter().copied() {
                button {
                    key: "{label}",
                    onclick: {
                        let key = key.clone();
                        move |_| key(bytes)
                    },
                    "{label}"
                }
            }
        }
    }
}
