# Due items and checks

Recurring work — *is the access key still valid*, *did the backup run*, *is
the disk filling* — is a **check** that waits in a context's queue until its
time, wakes a worker, and is closed with **pass**, **fail** or **snooze** and a
note.

## Queueing

```
kudo-shim context enqueue <context> --due +2h "rotate the key"
kudo-shim context check <context> "the backup ran" \
    --pass "a snapshot newer than 26 hours" \
    --window "the last 26 hours" --due +1d
```

A time is `+30m`, `+2h`, `+1d`, or an RFC 3339 time. Before it, the item is
not ready: no worker pulls it and no handoff passes it on. Once due, it goes
before items with no due time. `context show` counts the items not yet due
and names the next one's time.

## Dependencies and blocked work

Name earlier items with repeated `--after` flags when their work must submit
first:

```
kudo-shim context enqueue <context> "land the schema change"
kudo-shim context enqueue <context> --after <item> "update the client"
```

A dependent item is **waiting**, not ready, until every named item has submitted.
Dependencies must be distinct existing items in the same context; release and
blocked outcomes do not satisfy them.

A worker that discovers a prerequisite during an attempt calls `submit` with
`blocked_on` set to one or more item ids. This records the attempt and its log,
settles the claim as **blocked**, and leaves the head unchanged. The item waits
for those submissions rather than disappearing from the queue. Use `release`
instead when the item should become ready immediately.

## Closing a check

A worker handed a check sees what passes it and the evidence window, and
closes it through `submit` with a `verdict` — `pass`, `fail`, or `snooze` with
`snooze_until` — and a `note`. A check submitted without a verdict is refused,
and so is a verdict on an ordinary item. A snooze closes the item and queues
the same check again, due at that time; nothing repeats on its own, so a
recurring check is one the worker snoozes each time. The verdict is written
into the submission, so the context's log is the check log.

A tool that seems missing may only be still connecting: the worker's MCP
servers come up after its session starts. Every claim tells the worker to look
again, twice and a minute apart, before it snoozes, fails, releases or reports
the tool absent, and to say in its note that it re-probed. A note that names a
missing tool without that is worth a second look.

## Evidence

A check's result is usually "I ran X and saw Y". Each shell call a worker
makes is recorded as it returns — the command, its exit, and the first and
last 2 KB of its output — and the worker is told the kino to cite; a
governed job's start and end are recorded too, the end with the same excerpt
of what the job printed. A submission citing these is
**established**, not claimed. The record is in the context store (and its
mirror): whatever a command prints, secrets included, goes with it — keep
credentials out of output a worker runs.

## Waking

`kudo-shim waker` runs as a user service on one node. Once a minute it looks
for contexts with an item that came due and no worker running (no live lease,
no pane started in the last 15 minutes) and starts one, through the node's
typed worker start, on the context's **wake node**:

```
kudo-shim context wake-node <context> yolo
```

Without one, the node of the context's first repo (by name), else the
waker's own node. The waker starts workers only on the nodes in
`KUDO_CONSOLE_WORKER_NODES`, and never for an ordinary item: those wait for a
person to start a worker. A context it woke is not woken again for 15 minutes, then 30, doubling up
to 4 hours while its due item stays unanswered; a pane that never traced is
stopped before another starts. Once the lease of a
pane it started has ended, it stops that pane, so a finished worker's
session does not linger (it never stops a pane a person started); a pane
whose job is already gone holds nothing back. Woken workers default to the
waker's own node when it is one it may start on, else the first such node.
It refuses to run without `KUDO_CONSOLE_WORKER_NODES`. It needs `KUDO_HUB_ADDR`, and the mirror settings
(`KUDO_CONTEXT_MIRROR`, `AWS_*`) to see what other machines queued.
