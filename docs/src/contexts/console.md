# The console

`crates/kudo-console` is kudo's human surface: a Dioxus fullstack app. Its
server links the context engine (with this machine's mirror) and the hub
client; the page calls it through server functions.

On a wide screen the panes sit side by side: *where* (contexts with their
workers, and nodes), then either the selected context's **head** (with a box
to enqueue an ask) and **trace**, or the selected node's **jobs** and one
**job**'s output — and a taken-over worker's **terminal** beside either. A
top bar says where you are, steps back, and carries the approvals badge and
the theme toggle.

## On a phone

Below 720 px wide the console shows **one pane at a time**, the deepest thing
selected: the list; a context (with *head* / *trace* tabs); a node's jobs; a
job's output; a taken-over pane. **‹ back** in the top bar — or the phone's
own back — steps out one level. The approvals badge stays in the top bar on
every level.

The selection lives in the URL fragment (`#c=<context>&t=trace`,
`#n=<node>&j=<job>`, `#p=<node>/<job>`, `&tray=1`), so a reload or a link
sent to your other device lands on the same place; a link that no longer
parses lands on the list. Nothing typed ever goes into the URL.

The theme follows the system unless you pick one: **◐** cycles system → dark →
light → system, remembered in that browser only.

## Running it

In the dev shell (`nix develop`), from the repository root:

```
dx serve -p kudo-console --web --fullstack      # build, run, live-reload
```

or run a built server directly:

```
dx build -p kudo-console --web --fullstack --release
cd target/dx/kudo-console/release/web && ./server
```

Use the release build for anything reached over a network: the debug build's
WebAssembly is about 50 MB, and until it loads the page shows but no button
works. The release build's is about 2.4 MB.

It reads the same environment as `kudo-shim`: `KUDO_CONTEXT_STORE` (or the
default store), `KUDO_CONTEXT_MIRROR` and `AWS_*` for a mirror, and
`KUDO_HUB_ADDR` for nodes and live updates. It listens on `IP`:`PORT`,
**`127.0.0.1:8080` by default**.

## It has no login — keep it on localhost

v1 has **no authentication**: anyone who can reach the server can read every
context and enqueue asks that workers will act on. It refuses what another web
page could make your browser send (a write that is not same-origin JSON), but
nothing stops a person on the network. So:

- leave `IP` unset (localhost), and reach it from another machine through an
  SSH tunnel — `ssh -L 8080:127.0.0.1:8080 <host>`, then open
  `http://127.0.0.1:8080`;
- never bind it to a shared or public address.

## Starting a worker, and taking it over

Under each context, **start a worker on `<node>`** starts `kudo-shim worker`
for that context as a terminal job on that node (in the context's repo there,
when it declares one — otherwise in `/tmp`, where the agent CLI first asks to
trust the folder). The console picks the worker's lease, records the pane,
and selects it: the **terminal** pane shows its screen (re-read every second)
and takes typing — a line with Enter, or a key alone (the arrows, Tab, Enter,
Esc, Ctrl-C, Ctrl-S; the arrows and Esc also work from the empty line field),
sent in order. A worker
whose CLI has not started its session yet shows as *starting*, and can be
taken over all the same — which is how you answer a startup prompt. One still
silent after ten minutes shows as *quiet* (stuck at a prompt, or dead), and
after a day it is no longer listed.

Only panes the console started can be typed into, and only by the console
(the node refuses input from anyone else).

Create a context with `kudo-shim context new --worktree <title>` when parallel
workers must not share a checkout. With a named repo, each lease then launches
in its own detached worktree based on the repo's local `main`, under the context
store's `worktrees` directory. Restarting the same lease reuses that directory,
including unfinished changes. Worktrees are never deleted automatically; review
and remove them deliberately. Contexts created without the option keep the
existing shared-directory behavior.

A worker with nothing ready **waits** for the next item instead of ending: it
shows as `wait` in the *where* pane, and an item queued later is handed to it.
It looks locally every 3 s and syncs the mirror every 30 s. About every nine
minutes of waiting it replies one idle line (a cheap turn)
and waits again; after `KUDO_IDLE_LIMIT` seconds in a row with nothing
(default 7200, on the node) its lease ends. `KUDO_IDLE_LIMIT=0` ends it as
soon as nothing is ready. To take over a waiting worker, interrupt (Esc) and
type. In Kimi, typing during a running turn queues the message; press **Ctrl-S**
to steer immediately, which releases the worker's open claim and ends its lease
as a takeover.

`KUDO_MAX_TURNS` is the maximum number of **queue items** one lease may submit,
not the number of agent turns within an item. The default is 20; after that many
submitted items the lease ends; a later worker starts with a fresh session.

### Which agent CLI runs

The node's own `KUDO_AGENT_HARNESS` selects `claude` (the default) or `kimi`;
the console's start request cannot change it. `KUDO_AGENT_CLI` is a separate
executable-path override. Both harnesses use the same claims, leases, evidence,
and console semantics.

Kimi interactive workers need util-linux's `script` (installed by kudo's Home
Manager module) and use `--auto` when the node enables bypass. Kimi print mode
always uses auto permissions, so `kudo-shim worker <context> --headless` with
the Kimi harness requires `KUDO_WORKER_BYPASS=1`; without it, launch is refused
rather than silently running unattended with prompts.

### Which nodes run workers, and how

`KUDO_CONSOLE_WORKER_NODES` (comma-separated node ids) limits where the
console starts workers: only those nodes are offered, and a start on any other
is refused. Unset, every node is offered.

Every node is governed, so the console does not start a worker with a
command line: it asks the node's `kudo-worker` connector for a worker on the
context, its repo there and a lease, and the node composes the rest (see
[Deployment](../process-manager/deployment.md)). A node too old to offer it is
refused — redeploy it.

Whether a worker's CLI asks before running tools is the **node's** decision,
never the console's: a node whose own environment has `KUDO_WORKER_BYPASS=1`
maps that choice to the selected harness (`--permission-mode bypassPermissions`
for Claude, `--auto` for interactive Kimi). No caller can set it: the worker's
environment is composed by the node. Set it only on a
sandboxed node — yolo, whose outbound traffic goes through OneCLI:

```nix
services.kudo-node.environment.KUDO_WORKER_BYPASS = "1";
```

and run the console with `KUDO_CONSOLE_WORKER_NODES=yolo`.

Which MCP servers a worker gets is the node's decision too. By default it gets
kudo's server and the user's own (chat, trackers, search — whatever the CLI
loads for that user), so a check can read them. A node whose own environment
has `KUDO_WORKER_STRICT_MCP=1` gives its workers kudo's server only
(`--strict-mcp-config` for Claude, an isolated MCP config for Kimi):

```nix
services.kudo-node.environment.KUDO_WORKER_STRICT_MCP = "1";
```

Either way, project servers are not offered. Claude declines servers from
`.mcp.json` in the repository and its ancestors. Kimi reads project servers from
`.kimi-code/mcp.json`; because Kimi cannot trust a workspace while declining
those servers, a repo declaring any is refused before launch rather than
silently loading them.

To reach the console by a name other than localhost (through a tunnel that
keeps a hostname), list it in `KUDO_CONSOLE_HOSTS` (comma-separated); any
other `Host` is refused, which also defeats DNS rebinding.

Behind an ingress that rewrites `Host` (the proxy's name reaches the console,
while the browser's `Origin` stays the public one), also list the public
origin in `KUDO_CONSOLE_ORIGINS`. For yolo's ingress:

```sh
PORT=8080 KUDO_CONSOLE_HOSTS=8080.yolo.incus \
  KUDO_CONSOLE_ORIGINS=https://yolo-8080.pc.yjpark.org ./server
```

The console still binds 127.0.0.1, where yolo's Caddy forwards. It has no
login, so anyone the ingress lets in can use it.

## Answering the gate

Every node is governed, so a command no rule allows waits for **your**
verdict. The **approvals badge** in the top bar says how many jobs wait
(amber); **unknown on N nodes** (red) when nothing is queued but a node's
queue could not be read; **none** only when every node answered. Tap it for
the **tray**: every waiting job, oldest first in an order that does not
shift under your finger — the argv, the node and directory, the context whose
worker asked, the owner, and the minutes left before it is denied — and the
nodes not answering, named in red.

The buttons are the four verdicts — **allow once**, **allow always** (writes
an exact-argv rule on that node), **deny once**, **deny always** — and **edit,
then allow**, which runs your corrected argv instead (shell-quoted: quote an
argument with spaces; an empty or unparsable edit is refused). The **always**
verdicts take a **second tap**: the first turns the button into the exact
rule it will write and the one node it binds; a tap elsewhere, or ten
seconds, cancels. The tray shows what the verdict led to — in red if it
failed — and stays open, even once the queue is empty; ×, Esc or back closes
it. A verdict that arrives too late runs nothing, and says so.

Only the console's verdict counts: a node refuses one from anyone else.

## Nodes and their jobs

Under **nodes** in the *where* pane, each node the hub reaches (▣; a peer
hub's node, `hub::node`, is ⇄). Selecting one lists its governed jobs —
waiting, then running, then ended, newest first — each with its state, argv,
directory, the verdict it ran under, and how long it has waited or run (an
ended one, when it started). Selecting a job shows its output, newest at the
bottom and followed while it runs (the last 64 KB; output the node dropped is
marked). A waiting or running job can be **killed**, after a second tap; the
console refuses a job the node no longer lists as live. The list is re-read
every three seconds while shown, the output every second.

Reading a node's jobs needs a node with the moco revision whose listing
carries each job's argv (`1842948` or later); an older node's jobs show as
unreadable — redeploy it.

## Live updates

The server subscribes to the hub's `kudo/` events and the page long-polls a
version they bump, so it re-reads only when something moved (and at least
every 25 seconds). A hub without the event channel (older than the
`828a6b0` revision) still serves nodes; the page then refreshes on that
25-second beat.
