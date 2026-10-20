// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// HERDR_INTEGRATION_ID=opencode-tui
// HERDR_INTEGRATION_VERSION=14

import net from "node:net";

const SOURCE = "herdr:opencode";
const AGENT = "opencode";
const ROUTE_POLL_INTERVAL_MS = 100;
const SELECTION_RETRY_DELAYS_MS = [100, 400, 1_000];

function requestOnce(sessionID, state, seq, isCurrent = () => true) {
  const paneId = process.env.HERDR_PANE_ID;
  const socketPath = process.env.HERDR_SOCKET_PATH;
  if (!paneId || !socketPath) {
    return Promise.resolve(true);
  }

  const socketEndpoint =
    process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
  const request = {
    id: `${SOURCE}:tui:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
      .toString()
      .padStart(6, "0")}`,
    method: state === undefined ? "pane.report_agent_session" : "pane.report_agent",
    params: {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      agent_session_id: sessionID,
      ...(state === undefined ? { session_start_source: "select" } : { state, seq }),
    },
  };

  return new Promise((resolve) => {
    let settled = false;
    let timer;
    const settle = (delivered) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.destroy();
      resolve(delivered);
    };
    const client = net.createConnection(socketEndpoint, () => {
      if (!isCurrent()) {
        settle(false);
        return;
      }
      client.write(`${JSON.stringify(request)}\n`);
    });

    // A plain timer, not socket.setTimeout, so a connection that never finishes
    // connecting still settles and cannot block later reports behind the queue.
    timer = setTimeout(() => settle(false), 500);
    timer.unref?.();
    client.on("data", () => settle(true));
    client.on("error", () => settle(false));
    client.on("end", () => settle(false));
    client.on("close", () => settle(false));
  });
}

export default {
  id: "herdr.opencode.session-selection",
  // Keep this plain object dependency-free: V1 and V2 expose different SDK
  // packages, but both loaders accept their own lifecycle entry on this object.
  setup,
  tui,
};

async function tui(api) {
  if (process.env.HERDR_ENV !== "1" || !process.env.HERDR_SOCKET_PATH || !process.env.HERDR_PANE_ID) return;

  let disposed = false;
  let context;
  let sequence = Date.now() * 1000;
  let chain = Promise.resolve();

  function routeID() {
    const route = api.route.current;
    return route?.name === "session" ? route.params?.sessionID : undefined;
  }

  function current(ctx) {
    return !disposed && context === ctx && routeID() === ctx.route;
  }

  async function read(ctx, request) {
    const result = await request({
      signal: AbortSignal.any([ctx.controller.signal, AbortSignal.timeout(5_000)]),
      throwOnError: true,
    });
    if (!current(ctx) || result?.data === undefined) throw new Error("session data unavailable");
    return result.data;
  }

  // The first selected session owns this pane's subtree. Browsing descendants
  // keeps that boundary; directly attaching to a child does not claim siblings.
  function root(ctx, id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id) && !ctx.deleted.has(id)) {
      seen.add(id);
      if (id === ctx.boundary) return id;
      const session = ctx.sessions.get(id) ?? api.state.session.get(id);
      if (!session) return;
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  async function resolveRoot(ctx, id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id) && !ctx.deleted.has(id)) {
      seen.add(id);
      if (id === ctx.boundary) return id;
      let session = ctx.sessions.get(id) ?? api.state.session.get(id);
      if (!session) {
        if (!ctx.lookups.has(id)) {
          const requestedID = id;
          const lookup = read(ctx, (options) => api.client.session.get({ sessionID: requestedID }, options));
          ctx.lookups.set(id, lookup);
          lookup.finally(() => ctx.lookups.delete(requestedID)).catch(() => {});
        }
        session = await ctx.lookups.get(id);
        if (ctx.deleted.has(id)) return;
        session = ctx.sessions.get(id) ?? session;
        if (session.id !== id) throw new Error("unexpected session identity");
        ctx.sessions.set(id, session);
      }
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  function owners(ctx) {
    return new Set([...ctx.statuses.keys(), ...[...ctx.blockers.values()].map((r) => r.sessionID)]);
  }

  function state(ctx) {
    if (ctx.settled) return "idle";
    for (const request of ctx.blockers.values()) {
      if (root(ctx, request.sessionID) === ctx.selected) return "blocked";
    }
    if (ctx.errors.has(ctx.selected)) return "blocked";
    for (const id of ctx.statuses.keys()) {
      if (root(ctx, id) === ctx.selected) return "working";
    }
    if (ctx.hydrated && [...owners(ctx)].every((id) => root(ctx, id) !== undefined)) return "idle";
  }

  function publish(ctx, selection = false) {
    if (!current(ctx) || !ctx.selected) return;
    ctx.selectionPending ||= selection;
    if (ctx.queued) return;
    ctx.queued = true;
    chain = chain.then(async () => {
      ctx.queued = false;
      const selected = ctx.selected;
      const isCurrent = () => current(ctx) && !!selected && ctx.selected === selected &&
        (ctx.settled || root(ctx, ctx.route) === selected);
      if (!isCurrent()) return;
      if (ctx.selectionPending) {
        if (!await requestOnce(selected, undefined, undefined, isCurrent)) {
          ctx.retryAt = Date.now() + 500;
          return;
        }
        if (!isCurrent()) return;
        ctx.selectionPending = false;
        ctx.lastState = undefined;
      }
      const value = state(ctx);
      if (value === undefined || value === ctx.lastState) return;
      const delivered = await requestOnce(selected, value, ++sequence, isCurrent);
      if (!isCurrent()) return;
      ctx.lastState = delivered ? value : undefined;
      if (!delivered) ctx.retryAt = Date.now() + 500;
    }).catch(() => {
      if (current(ctx)) {
        ctx.lastState = undefined;
        ctx.retryAt = Date.now() + 500;
      }
    });
  }

  function retire(ctx, route) {
    ctx?.controller?.abort();
    context = ctx?.selected
      ? { route, selected: ctx.selected, settled: true, retryAt: Infinity }
      : undefined;
    if (context) publish(context);
  }

  function reconcile(ctx) {
    if (!current(ctx)) return;
    publish(ctx);
    if (ctx.resolving) {
      ctx.resolveAgain = true;
      return;
    }
    ctx.resolving = true;
    void (async () => {
      const ancestor = await resolveRoot(ctx, ctx.route);
      if (!current(ctx) || !ancestor) return;
      const selected = ancestor === ctx.boundary ? ctx.boundary : ctx.route;
      if (selected !== ctx.selected) {
        ctx.boundary = selected;
        ctx.selected = selected;
        publish(ctx, true);
      }
      // Fetch ancestry only for active/request-bearing sessions, not historical trees.
      await Promise.all([...owners(ctx)].map((id) => resolveRoot(ctx, id)));
      if (current(ctx)) publish(ctx);
    })().catch(() => {
      if (current(ctx)) ctx.retryAt = Date.now() + 500;
    }).finally(() => {
      ctx.resolving = false;
      if (ctx.resolveAgain) {
        ctx.resolveAgain = false;
        reconcile(ctx);
      }
    });
  }

  function clearRequests(ctx, matches) {
    for (const [key, request] of ctx.blockers) if (matches(request)) ctx.blockers.delete(key);
  }

  function terminalTool(part) {
    return part?.type === "tool" && ["completed", "error"].includes(part.state?.status);
  }

  function apply(ctx, event) {
    const data = event.properties;
    if (!data) return;
    if (event.type === "message.part.updated") {
      const part = data.part;
      if (terminalTool(part)) {
        clearRequests(ctx, (r) => r.sessionID === part.sessionID &&
          r.tool?.messageID === part.messageID && r.tool?.callID === part.callID);
      }
      return;
    }
    const id = data.sessionID ?? data.info?.id;
    if (typeof id !== "string" || ctx.deleted.has(id)) return;
    switch (event.type) {
      case "session.created":
      case "session.updated":
        if (data.info) ctx.sessions.set(id, data.info);
        break;
      case "session.deleted":
        ctx.deleted.add(id);
        ctx.sessions.delete(id);
        ctx.statuses.delete(id);
        ctx.errors.delete(id);
        clearRequests(ctx, (r) => r.sessionID === id);
        if (ctx.selected === id) retire(ctx, ctx.route);
        break;
      case "session.status":
      case "session.idle": {
        const status = event.type === "session.idle" ? "idle" : data.status?.type;
        if (status === "busy" || status === "retry") {
          ctx.statuses.set(id, status);
          ctx.errors.delete(id);
        } else if (status === "idle") {
          ctx.statuses.delete(id);
          ctx.errors.delete(id);
        }
        break;
      }
      case "session.error":
        if (data.error?.name !== "MessageAbortedError") ctx.errors.add(id);
        break;
      case "permission.asked":
      case "question.asked":
        if (typeof data.id === "string") ctx.blockers.set(`${event.type.split(".")[0]}:${data.id}`, data);
        break;
      case "permission.replied":
      case "question.replied":
      case "question.rejected":
        ctx.blockers.delete(`${event.type.split(".")[0]}:${data.requestID}`);
        break;
    }
  }

  async function hydrate(ctx) {
    if (!current(ctx) || ctx.loading) return;
    ctx.loading = true;
    try {
      const [statuses, permissions, questions] = await Promise.all([
        read(ctx, (options) => api.client.session.status(undefined, options)),
        read(ctx, (options) => api.client.permission.list(undefined, options)),
        read(ctx, (options) => api.client.question.list(undefined, options)),
      ]);
      if (!statuses || typeof statuses !== "object" || Array.isArray(statuses) ||
          !Object.values(statuses).every((s) => ["busy", "retry", "idle"].includes(s?.type)) ||
          !Array.isArray(permissions) || !Array.isArray(questions)) {
        throw new Error("incomplete session snapshot");
      }
      const messages = new Map();
      const blockers = new Map();
      let validated = true;
      await Promise.all([["permission", permissions], ["question", questions]].flatMap(([kind, requests]) =>
        requests.map(async (request) => {
          if (ctx.deleted.has(request.sessionID)) return;
          if (request.tool) {
            const { messageID, callID } = request.tool;
            const key = `${request.sessionID}:${messageID}`;
            if (!messages.has(key)) messages.set(key, read(ctx, (options) => api.client.session.message({
              sessionID: request.sessionID, messageID,
            }, options)));
            try {
              const message = await messages.get(key);
              const part = message.parts?.find((p) => p.type === "tool" && p.callID === callID);
              // OpenCode can retain pending requests after abort. Status alone is
              // insufficient: the same session may already be running a new turn.
              if (terminalTool(part)) return;
            } catch {
              validated = false;
            }
          }
          if (!ctx.deleted.has(request.sessionID)) blockers.set(`${kind}:${request.id}`, request);
        })));
      if (!current(ctx)) return;
      ctx.statuses = new Map(Object.entries(statuses)
        .filter(([id, status]) => !ctx.deleted.has(id) && (status.type === "busy" || status.type === "retry")));
      ctx.blockers = blockers;
      // Preserve deltas until hydration succeeds, including retry backoff:
      // a later snapshot must not revive a replied or cancelled request.
      for (const event of ctx.events) apply(ctx, event);
      ctx.hydrated = validated;
      if (!validated) ctx.retryAt = Date.now() + 500;
      reconcile(ctx);
    } catch {
      if (current(ctx)) ctx.retryAt = Date.now() + 500;
    } finally {
      ctx.loading = false;
      if (ctx.hydrated) ctx.events = [];
    }
  }

  function syncSelection(reset = false) {
    if (disposed) return;
    const id = routeID();
    if (reset || id !== context?.route) {
      if (typeof id !== "string" || !id) {
        retire(context, id);
        return;
      }
      const boundary = context?.settled ? undefined : context?.selected;
      context?.controller?.abort();
      const ctx = {
        route: id, boundary, controller: new AbortController(), sessions: new Map(), lookups: new Map(),
        statuses: new Map(), blockers: new Map(), errors: new Set(), deleted: new Set(),
        events: [], hydrated: false, loading: false, resolving: false,
        retryAt: Infinity, selectionAt: 0, retryIndex: 0,
      };
      context = ctx;
      reconcile(ctx);
      void hydrate(ctx);
    }
    const ctx = context;
    if (!ctx) return;
    if (!ctx.settled && ctx.selected && Date.now() >= ctx.selectionAt) {
      publish(ctx, true);
      const delay = SELECTION_RETRY_DELAYS_MS[ctx.retryIndex++];
      ctx.selectionAt = delay === undefined ? Infinity : Date.now() + delay;
    }
    if (Date.now() >= ctx.retryAt) {
      ctx.retryAt = Infinity;
      if (ctx.settled) publish(ctx);
      else {
        if (!ctx.hydrated) void hydrate(ctx);
        reconcile(ctx);
      }
    }
  }

  const subscriptions = [
    "session.created", "session.updated", "session.deleted", "session.status", "session.idle", "session.error",
    "permission.asked", "permission.replied", "question.asked", "question.replied", "question.rejected",
    "message.part.updated",
  ].map((type) => api.event.on(type, (event) => {
    if (type === "message.part.updated" && !terminalTool(event.properties?.part)) return;
    syncSelection();
    const ctx = context;
    if (!ctx || ctx.settled) return;
    if (ctx.loading || !ctx.hydrated) ctx.events.push(event);
    apply(ctx, event);
    reconcile(ctx);
  }));
  for (const type of ["server.connected", "server.instance.disposed", "global.disposed"]) {
    subscriptions.push(api.event.on(type, () => syncSelection(true)));
  }
  const poll = setInterval(syncSelection, ROUTE_POLL_INTERVAL_MS);
  // A host that also exposes the live pending APIs gets authoritative flags;
  // otherwise the bridge keeps the tab list's own values.
  const bridgePending = createPendingReader(api);
  const bridge = createTabBridge(api, { pending: bridgePending });
  api.lifecycle.onDispose(() => {
    disposed = true;
    context?.controller?.abort();
    clearInterval(poll);
    for (const unsubscribe of subscriptions) unsubscribe();
    bridge.dispose();
    // A bridge that never started returns an idle disposer, so release its
    // reader here as well. Disposing an already-closed reader is a no-op.
    bridgePending?.dispose?.();
  });
  syncSelection();
}

function setup(api) {
  if (process.env.HERDR_ENV !== "1" || !process.env.HERDR_SOCKET_PATH || !process.env.HERDR_PANE_ID) return;

  let disposed = false;
  let selected;
  let generation = 0;
  let sequence = Date.now() * 1000;
  let chain = Promise.resolve();
  let retryIndex = 0;
  let nextSelectionAt = 0;
  let retryTimer;
  const sessions = new Map();
  // Every session inside the selected root's subtree that is still running.
  // The selected session owns the pane, so a descendant keeps it working.
  const running = new Set();
  let blockers = new Map();
  // Event callbacks may precede cache updates. Retain each delta until the
  // cache reflects it, so late hydration cannot undo a reply or lose an ask.
  const blockerChanges = new Map();

  function root(id) {
    const seen = new Set();
    while (typeof id === "string" && !seen.has(id)) {
      seen.add(id);
      const session = api.data.session.get(id) ?? sessions.get(id);
      if (!session) return;
      if (!session.parentID) return id;
      id = session.parentID;
    }
  }

  function current() {
    const route = api.ui.router.current();
    return route.type === "session" ? root(route.sessionID) : undefined;
  }

  // Seed the running set from the live status cache. A pane that attaches while
  // descendants already run must start working instead of reporting idle.
  function runningFor(id) {
    const result = new Set();
    if (!id) return result;
    for (const member of [id, ...api.data.session.family(id)]) {
      if (root(member) === id && api.data.session.status(member) === "running") result.add(member);
    }
    return result;
  }

  // Selection and lifecycle use one queue. Recheck attribution at dispatch,
  // not just when receiving the event, and reject A -> B -> A stale work too.
  function enqueue(value) {
    const sessionID = selected;
    const revision = generation;
    const isCurrent = () => !disposed && revision === generation && !!sessionID && current() === sessionID;
    chain = chain.then(async () => {
      if (!isCurrent()) return;
      const delivered = await requestOnce(sessionID, value, value === undefined ? undefined : ++sequence, isCurrent);
      if (!delivered) scheduleStateRetry();
    }).catch(() => {});
  }

  // A dropped report must not strand the pane on a stale state once the
  // selection retry schedule has run out: resend the latest state until the
  // socket accepts it or the selection is no longer current.
  function scheduleStateRetry() {
    if (disposed || retryTimer) return;
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      publish();
    }, 500);
    retryTimer.unref?.();
  }

  // Blockers outrank everything. Only a real pending request (permission/form)
  // marks the pane blocked; a failed execution is not an input request, so it
  // must not pin the pane on blocked. Any running session inside the selected
  // subtree keeps the pane working, even after an individual child finishes.
  function sessionState() {
    if (blockers.size) return "blocked";
    if (running.size) return "working";
    return "idle";
  }

  function publish() {
    enqueue(sessionState());
  }

  function changeBlocker(id, kind, requestID, present) {
    if (typeof requestID !== "string") return;
    const key = `${kind}:${requestID}`;
    blockerChanges.set(key, { id, kind, requestID, present, at: Date.now() });
    if (present) blockers.set(key, id);
    else blockers.delete(key);
  }

  // Authoritative reads are shared with the tab bridge. When the host exposes
  // them, the cached list is never the source of truth for blockers.
  const pending = createPendingReader(api);

  function pendingMembers() {
    if (!selected) return [];
    return sessionFamily(api, selected).filter((member) => root(member) === selected);
  }

  function refreshPending() {
    if (disposed || !pending) return;
    const members = pendingMembers();
    if (members.length === 0) return;
    void pending.refresh(members)
      .then(() => {
        if (disposed) return;
        if (reconcileBlockers()) publish();
      })
      .catch(() => {});
  }

  function reconcileBlockers() {
    const next = new Map();
    const hydrated = new Set();
    const members = new Set([selected, ...api.data.session.family(selected), ...blockers.values()]);
    for (const member of members) {
      if (!member || root(member) !== selected) continue;
      for (const kind of ["permission", "form"]) {
        if (pending) {
          const entry = pending.pending(member);
          const kindEntry = entry ? entry[kind] : undefined;
          const value = kindEntry ? kindEntry.value : undefined;
          if (value === undefined) {
            // Nothing authoritative yet. Fall back to the local list so a real
            // blocker is not hidden while the first read is in flight; a list
            // that throws also keeps what is already held.
            let items;
            try {
              items = api.data.session[kind].list(member);
            } catch {
              items = undefined;
            }
            if (!Array.isArray(items)) {
              for (const [key, owner] of blockers) {
                if (owner === member && key.startsWith(`${kind}:`)) next.set(key, owner);
              }
              continue;
            }
            for (const item of items) {
              if (pending.isSuppressed(member, kind, item?.id)) continue;
              next.set(`${kind}:${item.id}`, member);
            }
            continue;
          }
          hydrated.add(`${kind}:${member}`);
          if (Array.isArray(kindEntry.ids)) {
            // Exact request ids, so a reply removes only its own request.
            for (const id of kindEntry.ids) next.set(`${kind}:${id}`, member);
          } else if (value) {
            next.set(`${kind}:authoritative:${member}`, member);
          }
          continue;
        }
        let items;
        try {
          items = api.data.session[kind].list(member);
        } catch {
          items = undefined;
        }
        if (!Array.isArray(items)) {
          for (const [key, owner] of blockers) {
            if (owner === member && key.startsWith(`${kind}:`)) next.set(key, owner);
          }
          continue;
        }
        hydrated.add(`${kind}:${member}`);
        for (const item of items) next.set(`${kind}:${item.id}`, member);
      }
    }
    for (const [key, change] of blockerChanges) {
      const expired = Date.now() - (change.at ?? 0) > BRIDGE_BLOCKER_DELTA_TTL_MS;
      if (!change.present) {
        // A reply removes exactly its own request, even when the read that
        // revealed it started earlier. With an authoritative reader, tombstone
        // it for every root: an old read for an inactive root, or one racing a
        // selection switch, must not resurrect the request later.
        if (pending) {
          if (typeof change.requestID === "string") {
            pending.drop(change.id, change.kind, change.requestID);
          }
          blockerChanges.delete(key);
          if (root(change.id) === selected) next.delete(key);
          continue;
        }
        if (root(change.id) !== selected) {
          // Without a reader, keep the reply until the root is selected and its
          // list confirms the absence, or until the bounded TTL ends.
          if (expired) blockerChanges.delete(key);
          continue;
        }
        const stillListed = next.has(key);
        next.delete(key);
        // Keep the tombstone while the cached list still reports the request.
        // Deleting it here would let the next reconcile re-add a stale row.
        if ((hydrated.has(`${change.kind}:${change.id}`) && !stillListed) || expired) {
          blockerChanges.delete(key);
        }
        continue;
      }
      const entry = pending ? pending.pending(change.id) : undefined;
      const kindEntry = entry ? entry[change.kind] : undefined;
      const value = kindEntry ? kindEntry.value : undefined;
      // A read that STARTED after this delta observed it at the source. Using the
      // start time (not completion) means a fetch racing a fresh ask never
      // discards the ask. A failed kind read keeps its old startedAt, so the
      // preserved value can never supersede a newer ask.
      const readCoversChange = !!kindEntry && value !== undefined &&
        kindEntry.startedAt > (change.at ?? 0);
      if (readCoversChange) {
        // The read started after this delta, so its ids already reflect it.
        blockerChanges.delete(key);
        continue;
      }
      if (expired) {
        // Never confirmed and too old to keep claiming: a stale cache cannot
        // hold the pane on blocked forever.
        blockerChanges.delete(key);
      } else {
        next.set(key, change.id);
      }
    }
    const changed = (blockers.size > 0) !== (next.size > 0);
    blockers = next;
    return changed;
  }

  function syncSelection() {
    if (disposed) return;
    const id = current();
    if (id !== selected) {
      selected = id;
      generation += 1;
      retryIndex = 0;
      nextSelectionAt = 0;
      blockers.clear();
      // Reply tombstones outlive the selection: they protect shared reads and a
      // later return to this root from an answer that was already handled. A
      // pending ask belongs to the old root, so it must not leak forward.
      for (const [key, change] of blockerChanges) {
        if (change.present) blockerChanges.delete(key);
      }
      running.clear();
      // A selection can land on a subtree whose descendants already run.
      if (id) for (const member of runningFor(id)) running.add(member);
    }
    if (!id) return;
    const blockersChanged = reconcileBlockers();
    if (Date.now() < nextSelectionAt) {
      if (blockersChanged) publish();
      return;
    }
    enqueue(undefined);
    publish();
    const delay = SELECTION_RETRY_DELAYS_MS[retryIndex++];
    nextSelectionAt = delay === undefined ? Number.POSITIVE_INFINITY : Date.now() + delay;
  }

  function receive({ details: event }) {
    if (disposed) return;
    const data = event.data;
    if (data == null) return;
    if (event.type === "session.created") {
      sessions.set(data.sessionID, { id: data.sessionID, parentID: data.parentID });
    }
    if (event.type === "session.deleted") {
      const affected = data.sessionID === selected || running.has(data.sessionID) ||
        [...blockers.values()].includes(data.sessionID);
      sessions.delete(data.sessionID);
      running.delete(data.sessionID);
      // Deletion is delivered after the cache can remove the session. Use
      // stored ownership rather than looking up the deleted child's ancestry.
      for (const [key, owner] of blockers) if (owner === data.sessionID) blockers.delete(key);
      for (const [key, change] of blockerChanges) {
        if (change.id === data.sessionID) blockerChanges.delete(key);
      }
      syncSelection();
      if (selected && affected) publish();
      return;
    }
    syncSelection();
    const id = event.type === "form.created" ? data.form.sessionID : data.sessionID;
    // Replies are tombstoned before selection filtering: an old read for any
    // root can still be in flight, and a reply that lands around a selection
    // change must not resurrect its request later.
    const replyKind = event.type === "permission.replied" ? "permission" :
      (event.type === "form.replied" || event.type === "form.cancelled" ? "form" : undefined);
    if (replyKind) {
      const requestID = replyKind === "permission" ? data.requestID : data.id;
      if (pending && typeof requestID === "string") pending.drop(id, replyKind, requestID);
      changeBlocker(id, replyKind, requestID, false);
      // Only the selected root may change the pane state. Other roots still get
      // their tombstone so a stale answer cannot revive them.
      if (!selected || root(id) !== selected) return;
      publish();
      return;
    }
    if (!selected || root(id) !== selected) return;
    switch (event.type) {
      case "permission.asked":
        changeBlocker(id, "permission", data.id, true);
        break;
      case "form.created":
        changeBlocker(id, "form", data.form.id, true);
        break;
      case "session.execution.started":
        running.add(id);
        break;
      case "session.execution.succeeded":
      case "session.execution.interrupted":
        running.delete(id);
        break;
      case "session.execution.failed":
        running.delete(id);
        // A failed execution is not an input request: it stops the working
        // contribution but never marks the pane blocked on its own.
        break;
      default:
        return;
    }
    publish();
  }

  const unsubscribe = api.data.listen(receive);
  syncSelection();
  refreshPending();
  const poll = setInterval(() => {
    refreshPending();
    syncSelection();
  }, ROUTE_POLL_INTERVAL_MS);
  const bridge = createTabBridge(api, { pending });
  return () => {
    disposed = true;
    generation += 1;
    clearTimeout(retryTimer);
    clearInterval(poll);
    unsubscribe();
    sessions.clear();
    blockers.clear();
    running.clear();
    blockerChanges.clear();
    bridge.dispose();
    pending?.dispose?.();
  };
}

// ---------------------------------------------------------------------------
// Tab bridge
//
// Publishes this pane's OpenCode root tabs as pane metadata, so the Herdr
// sidebar can list them, and consumes Herdr focus requests for them. Only
// structured, bounded socket traffic is used. When the tab API is missing the
// bridge never starts and the original lifecycle behavior is untouched.
// ---------------------------------------------------------------------------

const BRIDGE_SOURCE = "herdr:opencode:tabs";
const BRIDGE_TABS_TOKEN = "herdr_oc_tabs";
const BRIDGE_FOCUS_TOKEN = "herdr_oc_focus";
const BRIDGE_VERSION = 1;
const BRIDGE_TABS_TTL_MS = 15_000;
const BRIDGE_POLL_INTERVAL_MS = 1_000;
const BRIDGE_RECONCILE_INTERVAL_MS = 1_000;
const BRIDGE_HEARTBEAT_MS = 5_000;
// A snapshot is only kept alive while the tab list still confirms it. Missing
// this grace window (three missed polls) counts as a persistent failure: the
// heartbeat stops and the token is left to expire, so closed rows disappear.
const BRIDGE_VALIDATION_GRACE_MS = 3_000;
const BRIDGE_SOCKET_TIMEOUT_MS = 500;
const BRIDGE_SUBSCRIBE_ACK_TIMEOUT_MS = 500;
const BRIDGE_FOCUS_WATCHDOG_MS = 5_000;
// Sentinel for a focus call that never settled. It is not a cancellation: the
// host call stays outstanding and the drain stays closed until it settles.
const BRIDGE_FOCUS_STALLED = Symbol("focus stalled");
const BRIDGE_BACKOFF_MS = 500;
const BRIDGE_BACKOFF_MAX_MS = 5_000;
const BRIDGE_API_TIMEOUT_MS = 2_000;
// Bounded, not truncated: a list beyond these limits is rejected as a whole.
const BRIDGE_MAX_TABS = 64;
const BRIDGE_MAX_ID_SCALARS = 128;
const BRIDGE_MAX_TITLE_SCALARS = 128;
const BRIDGE_MAX_PAYLOAD_BYTES = 65_536;
const BRIDGE_MAX_LINE_BYTES = 1_048_576;
const BRIDGE_FOCUS_DEDUPE_MS = 60_000;
const BRIDGE_FOCUS_DEDUPE_MAX = 256;
const BRIDGE_FOCUS_FUTURE_MS = 300_000;
const BRIDGE_ID_PATTERN = /^[A-Za-z0-9_-]+$/;
const BRIDGE_FOCUS_REQUEST_PATTERN = /^[0-9a-f]{32}$/;
const BRIDGE_CONTROL_PATTERN =
  /[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u2028\u2029\u202a-\u202e\u2060\ufeff]/;
// Blocker reconciliation asks the live API rather than trusting a cache that a
// dropped event can leave stale. Freshness dedupes the pane and tab readers.
const BRIDGE_PENDING_FRESH_MS = 750;
const BRIDGE_PENDING_MAX_SESSIONS = 32;
const BRIDGE_PENDING_SYNC_TIMEOUT_MS = 1_500;
const BRIDGE_PENDING_TICK_BUDGET_MS = 2_000;
const BRIDGE_PENDING_TIMEOUT = Symbol("pending read timed out");
// An event delta that no authoritative or cached read ever confirms must not
// pin the pane on blocked forever.
const BRIDGE_BLOCKER_DELTA_TTL_MS = 15_000;

function bridgeEndpoint() {
  const socketPath = process.env.HERDR_SOCKET_PATH;
  if (!socketPath) return undefined;
  return process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
}

function bridgeOwner() {
  const bytes = new Uint8Array(16);
  try {
    globalThis.crypto.getRandomValues(bytes);
  } catch {
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

// The tab surface is public API, but V1 and V2 hosts expose it at different
// depths. Any host without a complete list/focus pair keeps the old behavior.
function bridgeTabs(api) {
  for (const tabs of [api?.context?.ui?.tabs, api?.ui?.tabs, api?.tabs]) {
    if (tabs && typeof tabs.list === "function" && typeof tabs.focus === "function") return tabs;
  }
  return undefined;
}

function bridgeTitle(value) {
  if (typeof value !== "string") return "";
  let title = "";
  let scalars = 0;
  for (const character of value) {
    if (scalars >= BRIDGE_MAX_TITLE_SCALARS) break;
    if (BRIDGE_CONTROL_PATTERN.test(character)) continue;
    title += character;
    scalars += 1;
  }
  return title;
}

// `undefined` means "the list could not be trusted": keep the previous token
// rather than publish something partial. `[]` is a real, empty snapshot.
// `live` may override the per-row flags with values read from the live APIs.
function bridgeTabsSnapshot(raw, owner, live) {
  if (!Array.isArray(raw)) return undefined;
  const tabs = [];
  const seen = new Set();
  let active = 0;
  for (const entry of raw) {
    const sessionID = typeof entry?.sessionID === "string" ? entry.sessionID : "";
    // A row without a usable `sessionID` (absent, or empty after trimming) is a
    // host placeholder such as an unnamed "New session". Drop the row and keep
    // the valid siblings; only a nonblank ID that breaks the id contract makes
    // the whole list untrustworthy.
    if (sessionID.trim().length === 0) continue;
    if ([...sessionID].length > BRIDGE_MAX_ID_SCALARS) return undefined;
    if (!BRIDGE_ID_PATTERN.test(sessionID)) return undefined;
    if (seen.has(sessionID)) return undefined;
    if (tabs.length >= BRIDGE_MAX_TABS) return undefined;
    seen.add(sessionID);
    const isActive = entry.active === true;
    if (isActive) active += 1;
    tabs.push({
      sessionID,
      title: bridgeTitle(entry.title),
      active: isActive,
      busy: entry.busy === true,
      attention: entry.attention === true,
      unread: entry.unread === true,
    });
  }
  if (active > 1) return undefined;
  if (typeof live === "function") {
    for (const tab of tabs) {
      const flags = live(tab.sessionID);
      if (!flags) continue;
      if (typeof flags.busy === "boolean") tab.busy = flags.busy;
      if (typeof flags.attention === "boolean") tab.attention = flags.attention;
    }
  }
  const payload = JSON.stringify({ v: BRIDGE_VERSION, owner, tabs });
  if (new TextEncoder().encode(payload).length > BRIDGE_MAX_PAYLOAD_BYTES) return undefined;
  return { tabs, payload };
}

function bridgeSnapshotOwner(value) {
  if (typeof value !== "string" || !value) return undefined;
  try {
    const parsed = JSON.parse(value);
    if (typeof parsed?.owner !== "string") return undefined;
    return { owner: parsed.owner, empty: !Array.isArray(parsed.tabs) || parsed.tabs.length === 0 };
  } catch {
    return undefined;
  }
}

// The backend stamps `expiresAtMs` before storage, so compare meaning, not
// bytes: a truncated or rewritten token must never enable focus.
function bridgeMatchesSnapshot(value, owner, tabs) {
  if (typeof value !== "string" || !value) return false;
  let parsed;
  try {
    parsed = JSON.parse(value);
  } catch {
    return false;
  }
  if (parsed?.v !== BRIDGE_VERSION || parsed.owner !== owner) return false;
  if (!Array.isArray(parsed.tabs) || parsed.tabs.length !== tabs.length) return false;
  return parsed.tabs.every((tab, index) => {
    const want = tabs[index];
    return tab?.sessionID === want.sessionID &&
      tab.title === want.title &&
      tab.active === want.active &&
      tab.busy === want.busy &&
      tab.attention === want.attention &&
      tab.unread === want.unread;
  });
}

function bridgeFocusRequest(raw, owner, now) {
  if (typeof raw !== "string" || !raw) return undefined;
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return undefined;
    if (parsed.v !== BRIDGE_VERSION) return undefined;
    if (parsed.owner !== owner) return undefined;
    if (typeof parsed.sessionID !== "string" || !BRIDGE_ID_PATTERN.test(parsed.sessionID)) return undefined;
    if (parsed.sessionID.length > BRIDGE_MAX_ID_SCALARS) return undefined;
    if (!BRIDGE_FOCUS_REQUEST_PATTERN.test(parsed.requestID ?? "")) return undefined;
    const expiry = parsed.expiresAtMs;
    if (typeof expiry !== "number" || !Number.isFinite(expiry)) return undefined;
    if (now >= expiry) return undefined;
    if (expiry - now > BRIDGE_FOCUS_FUTURE_MS) return undefined;
    return { sessionID: parsed.sessionID, requestID: parsed.requestID };
  } catch {
    return undefined;
  }
}

function withDeadline(promise, timeoutMs) {
  let timer;
  const deadline = new Promise((resolve) => {
    timer = setTimeout(() => resolve(undefined), timeoutMs);
    timer.unref?.();
  });
  return Promise.race([promise, deadline]).finally(() => clearTimeout(timer));
}

// Herdr answers with {"id":..,"result":{"type":"<snake_case>",..}} on success and
// {"id":..,"error":{"code","message"}} on failure. Only the former counts: a line
// that merely echoes this request's id, or carries an unshaped `result`, is a
// failure, never a success.
function bridgeSuccessResult(message) {
  if (!message || typeof message !== "object") return undefined;
  if (message.error) return undefined;
  const result = message.result;
  if (!result || typeof result !== "object") return undefined;
  if (typeof result.type !== "string" || !result.type) return undefined;
  return result;
}

// The live API is authoritative for blockers. The event stream can drop or
// reorder notifications, and the local cache is only as good as those events.
function sessionFamily(api, rootID) {
  if (typeof rootID !== "string" || !rootID) return [];
  const family = api?.data?.session?.family;
  const ids = typeof family === "function" ? family.call(api.data.session, rootID) : [];
  const result = [rootID];
  if (Array.isArray(ids)) {
    for (const id of ids) if (typeof id === "string" && id !== rootID && !result.includes(id)) result.push(id);
  }
  return result;
}

// One reader per hook instance, so the pane and the tab bridge share answers.
// A member is not asked again within BRIDGE_PENDING_FRESH_MS, which keeps the
// per-second reconciliation from multiplying requests.
function createPendingReader(api) {
  const permission = api?.data?.session?.permission;
  const form = api?.data?.session?.form;
  if (typeof permission?.sync !== "function" || typeof permission?.list !== "function") return undefined;
  if (typeof form?.sync !== "function" || typeof form?.list !== "function") return undefined;
  const entries = new Map();
  const inflight = new Set();
  // request ids answered by this client but not yet confirmed by a read that
  // started after the reply. They are filtered out of stale reads so a preserved
  // list cannot resurrect an answered request.
  const replies = new Map();
  let closed = false;
  const location = () => {
    try {
      return api?.data?.location?.default?.() ?? api?.location;
    } catch {
      return undefined;
    }
  };
  // A sync that never settles must not wedge the poll or leave a member marked
  // in flight. The timeout result is discarded, and a late answer is never
  // committed: only the awaited outcome can mutate an entry.
  async function read(run) {
    if (closed) return undefined;
    let timer;
    const deadline = new Promise((resolve) => {
      timer = setTimeout(() => resolve(BRIDGE_PENDING_TIMEOUT), BRIDGE_PENDING_SYNC_TIMEOUT_MS);
      timer.unref?.();
    });
    try {
      const outcome = await Promise.race([run(), deadline]);
      if (outcome === BRIDGE_PENDING_TIMEOUT || closed) return undefined;
      return outcome;
    } catch {
      return undefined;
    } finally {
      clearTimeout(timer);
    }
  }
  let cursor = 0;
  async function refresh(members) {
    if (closed) return;
    const at = Date.now();
    const budget = at + BRIDGE_PENDING_TICK_BUDGET_MS;
    const unique = [...new Set(members.filter((member) => typeof member === "string" && member))];
    if (unique.length === 0) return;
    // Bounded per tick, and rotating so a large tab set cannot starve the tail.
    const ordered = unique.slice(cursor).concat(unique.slice(0, cursor));
    cursor = (cursor + BRIDGE_PENDING_MAX_SESSIONS) % unique.length;
    for (const member of ordered.slice(0, BRIDGE_PENDING_MAX_SESSIONS)) {
      if (closed) return;
      // The whole pass is time-budgeted, so many slow members cannot stretch a
      // refresh for minutes.
      if (Date.now() >= budget) break;
      const previous = entries.get(member);
      if (previous && at - previous.at < BRIDGE_PENDING_FRESH_MS) continue;
      if (inflight.has(member)) continue;
      inflight.add(member);
      const startedAt = Date.now();
      // Only a definitive read replaces a kind. A failure keeps the previous
      // answer AND its original startedAt, so an old value can never look fresh.
      const next = {
        permission: previous?.permission,
        form: previous?.form,
        at: previous?.at ?? 0,
      };
      try {
        const items = await read(async () => {
          await permission.sync(member);
          return permission.list(member);
        });
        if (Array.isArray(items)) next.permission = describeKind(member, "permission", items, startedAt);
        // Do not issue the second SDK call once the reader has been disposed.
        if (closed) return;
        const forms = await read(async () => {
          await form.sync(member, location());
          return form.list(member, location());
        });
        if (Array.isArray(forms)) next.form = describeKind(member, "form", forms, startedAt);
      } finally {
        inflight.delete(member);
      }
      if (closed) return;
      const changed = next.permission !== previous?.permission || next.form !== previous?.form;
      if (changed) next.at = Date.now();
      entries.set(member, next);
    }
    if (entries.size > 256) {
      for (const key of entries.keys()) {
        if (entries.size <= 256) break;
        if (!inflight.has(key)) entries.delete(key);
      }
    }
  }
  // Keeps the exact request ids so a reply can remove only its own request, and
  // filters ids a read that started before the reply could still report.
  function describeKind(member, kind, items, startedAt) {
    let ids = items.map((item) => item?.id).filter((id) => typeof id === "string" && id);
    const kinds = replies.get(member);
    const set = kinds?.get(kind);
    if (set) {
      for (const [id, at] of [...set]) {
        if (startedAt > at) {
          // This read began after the reply, so the server answer is final.
          set.delete(id);
          continue;
        }
        ids = ids.filter((value) => value !== id);
      }
      if (set.size === 0) kinds.delete(kind);
      if (kinds.size === 0) replies.delete(member);
    }
    return { value: ids.length > 0, ids, startedAt };
  }
  function drop(member, kind, requestID) {
    if (typeof member !== "string" || typeof kind !== "string") return;
    if (typeof requestID !== "string" || !requestID) return;
    let kinds = replies.get(member);
    if (!kinds) {
      kinds = new Map();
      replies.set(member, kinds);
    }
    let set = kinds.get(kind);
    if (!set) {
      set = new Map();
      kinds.set(kind, set);
    }
    set.set(requestID, Date.now());
    const entry = entries.get(member);
    const kindEntry = entry?.[kind];
    if (kindEntry && Array.isArray(kindEntry.ids) && kindEntry.ids.includes(requestID)) {
      kindEntry.ids = kindEntry.ids.filter((value) => value !== requestID);
      kindEntry.value = kindEntry.ids.length > 0;
    }
  }
  function isSuppressed(member, kind, requestID) {
    return !!replies.get(member)?.get(kind)?.has(requestID);
  }
  function pending(member) {
    return entries.get(member);
  }
  // true: at least one member has an open request. false: every member and both
  // kinds were read definitively with nothing open. undefined: either kind is
  // unconfirmed, so nothing may be cleared.
  function attention(members) {
    let unknown = false;
    for (const member of members) {
      const entry = entries.get(member);
      if (!entry) {
        unknown = true;
        continue;
      }
      if (entry.permission?.value === true || entry.form?.value === true) return true;
      if (entry.permission?.value === undefined || entry.form?.value === undefined) unknown = true;
    }
    return unknown ? undefined : false;
  }
  function dispose() {
    closed = true;
    inflight.clear();
    entries.clear();
    replies.clear();
  }
  return { refresh, pending, attention, drop, isSuppressed, dispose };
}

function createTabBridge(api, options = {}) {
  const idle = { dispose() {} };
  const tabs = bridgeTabs(api);
  if (!tabs) return idle;
  const endpoint = bridgeEndpoint();
  const paneId = process.env.HERDR_PANE_ID;
  if (!endpoint || !paneId) return idle;

  const owner = bridgeOwner();
  const pending = options.pending;
  let disposed = false;
  let owned = false;
  let focusEnabled = false;
  let focusBroken = false;
  let sequence = Date.now() * 1000;
  let lastPublished;
  let lastTabs;
  let lastPublishedAt = 0;
  let lastValidated;
  let lastValidatedAt = 0;
  let lastFocusRaw;
  let polling = false;
  let observing = false;
  let streaming = false;
  let queue;
  let backoff = BRIDGE_BACKOFF_MS;
  let eventSocket;
  let eventTimer;
  let eventBuffer = "";
  const recentFocus = new Map();
  let pendingFocus;
  let focusDraining = false;
  // Bumped whenever a focus settles, so list reads that started earlier cannot
  // publish a snapshot older than the focus they raced with.
  let focusEpoch = 0;

  // One bounded request/response exchange. The response must be structured
  // NDJSON carrying this request's id; raw bytes are never treated as success.
  function call(method, params, timeoutMs = BRIDGE_SOCKET_TIMEOUT_MS) {
    return new Promise((resolve) => {
      if (disposed) {
        resolve(undefined);
        return;
      }
      const id = `${BRIDGE_SOURCE}:${Date.now()}:${++sequence}`;
      const line = `${JSON.stringify({
        id,
        method,
        params: { pane_id: paneId, source: BRIDGE_SOURCE, agent: AGENT, ...params },
      })}\n`;
      let settled = false;
      let buffer = "";
      let timer;
      const finish = (value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        try {
          socket.destroy();
        } catch {}
        resolve(value);
      };
      const socket = net.createConnection(endpoint, () => {
        if (settled || disposed) {
          finish(undefined);
          return;
        }
        socket.write(line);
      });
      timer = setTimeout(() => finish(undefined), timeoutMs);
      timer.unref?.();
      socket.on("data", (chunk) => {
        if (settled) return;
        buffer += chunk;
        if (buffer.length > BRIDGE_MAX_LINE_BYTES) {
          finish(undefined);
          return;
        }
        let index;
        while ((index = buffer.indexOf("\n")) !== -1) {
          const raw = buffer.slice(0, index);
          buffer = buffer.slice(index + 1);
          let message;
          try {
            message = JSON.parse(raw);
          } catch {
            continue;
          }
          if (!message || message.id !== id) continue;
          finish(bridgeSuccessResult(message));
          return;
        }
      });
      socket.on("error", () => finish(undefined));
      socket.on("end", () => finish(undefined));
      socket.on("close", () => finish(undefined));
    });
  }

  // Focus control is a Core pane token, so it is only readable through
  // pane.current. A missing or truncated readback disables focus for good.
  async function verifyReadback(expected) {
    const result = await call("pane.current", { caller_pane_id: paneId });
    if (disposed) return;
    const value = result?.pane?.tokens?.[BRIDGE_TABS_TOKEN];
    if (typeof value !== "string") return;
    if (bridgeMatchesSnapshot(value, owner, expected)) focusEnabled = true;
    else focusBroken = true;
  }

  // The backend only keeps a token for the TTL it was granted, so an identical
  // payload has to be republished once that window has passed: the stored token
  // is gone even though nothing about the snapshot changed.
  const tokenLapsed = () => Date.now() - lastPublishedAt >= BRIDGE_TABS_TTL_MS;

  async function drain() {
    streaming = true;
    while (!disposed && queue) {
      const job = queue;
      queue = undefined;
      if (!job.force && job.payload === lastPublished && !tokenLapsed()) continue;
      if (!job.nonempty && !owned) continue;
      const result = await call("pane.report_metadata", {
        tokens: { [BRIDGE_TABS_TOKEN]: job.payload },
        seq: ++sequence,
        ttl_ms: BRIDGE_TABS_TTL_MS,
      });
      if (disposed) return;
      if (result === undefined) continue;
      lastPublished = job.payload;
      lastTabs = job.tabs;
      lastPublishedAt = Date.now();
      owned = job.nonempty;
      if (job.nonempty && !focusEnabled && !focusBroken) await verifyReadback(job.tabs);
    }
    streaming = false;
  }

  function queuePublish(snapshot, force = false) {
    if (disposed) return;
    queue = { payload: snapshot.payload, tabs: snapshot.tabs, nonempty: snapshot.tabs.length > 0, force };
    if (!streaming) void drain().catch(() => {});
  }

  // An unchanged snapshot still expires, but only while the tab list keeps
  // confirming it. A list that has been unusable for a whole grace window is
  // persistent failure: the refresh stops and the token is left to expire so
  // rows for closed tabs do not live on. It resumes as soon as the list is
  // valid again, even when the payload is byte-identical.
  function heartbeat() {
    if (disposed || !owned) return;
    if (typeof lastPublished !== "string" || !lastTabs) return;
    if (!lastValidated || lastValidated.payload !== lastPublished) return;
    if (Date.now() - lastValidatedAt >= BRIDGE_VALIDATION_GRACE_MS) return;
    if (Date.now() - lastPublishedAt < BRIDGE_HEARTBEAT_MS) return;
    queue = {
      payload: lastValidated.payload,
      tabs: lastValidated.tabs,
      nonempty: true,
      force: true,
    };
    if (!streaming) void drain().catch(() => {});
  }

  // Row flags come from the live host APIs, not from the tab list's cached
  // mirror: a dropped event must not leave a settled tab blocked forever.
  // Unknown values keep the host's own flag (undefined leaves it untouched).
  function liveFlags(sessionID) {
    const status = api?.data?.session?.status;
    if (typeof status !== "function" && !pending) return undefined;
    const members = sessionFamily(api, sessionID);
    let busy;
    if (typeof status === "function") {
      try {
        let known = false;
        let unknown = false;
        for (const member of members) {
          const value = status.call(api.data.session, member);
          if (value === "running") {
            busy = true;
            break;
          }
          if (typeof value === "string") known = true;
          else unknown = true;
        }
        // A descendant with no status yet must not turn a working root idle.
        if (busy === undefined) busy = known && !unknown ? false : undefined;
      } catch {
        busy = undefined;
      }
    }
    return { busy, attention: pending ? pending.attention(members) : undefined };
  }

  function readTabs() {
    if (disposed) return Promise.resolve(undefined);
    let listed;
    try {
      listed = tabs.list();
    } catch {
      return Promise.resolve(undefined);
    }
    return withDeadline(Promise.resolve(listed), BRIDGE_API_TIMEOUT_MS)
      .then((value) => {
        if (disposed) return undefined;
        if (Array.isArray(value)) return value;
        if (Array.isArray(value?.data)) return value.data;
        if (Array.isArray(value?.tabs)) return value.tabs;
        return undefined;
      })
      .catch(() => undefined);
  }

  async function poll() {
    if (disposed || polling) return;
    polling = true;
    const epoch = focusEpoch;
    try {
      const raw = await readTabs();
      let snapshot = bridgeTabsSnapshot(raw, owner, liveFlags);
      if (pending && snapshot) {
        // Refresh every open family before the snapshot is trusted, so the
        // published flags reflect the API rather than the tab list's mirror.
        try {
          await pending.refresh(snapshot.tabs.flatMap((tab) => sessionFamily(api, tab.sessionID)));
        } catch (error) {
          diagnose(`blocker refresh failed: ${describeError(error)}`);
        }
        if (disposed || epoch !== focusEpoch) return;
        snapshot = bridgeTabsSnapshot(raw, owner, liveFlags);
      }
      if (disposed || !snapshot || epoch !== focusEpoch) return;
      lastValidated = { payload: snapshot.payload, tabs: snapshot.tabs };
      lastValidatedAt = Date.now();
      queuePublish(snapshot);
    } finally {
      polling = false;
    }
  }

  function rememberFocus(requestID) {
    recentFocus.set(requestID, Date.now());
    const cutoff = Date.now() - BRIDGE_FOCUS_DEDUPE_MS;
    for (const [key, at] of recentFocus) if (at < cutoff) recentFocus.delete(key);
    while (recentFocus.size > BRIDGE_FOCUS_DEDUPE_MAX) {
      recentFocus.delete(recentFocus.keys().next().value);
    }
  }

  // Focus failures must stay visible: a swallowed rejection looks like a dead
  // control. The message is fixed text plus an already-validated session id, so
  // it cannot grow with untrusted input.
  function diagnose(message) {
    try {
      console.warn(`[herdr:opencode:tabs] ${message}`);
    } catch {}
  }

  function describeError(error) {
    let text;
    try {
      text = error instanceof Error ? error.message : String(error);
    } catch {
      text = "unknown error";
    }
    return text.length > 160 ? `${text.slice(0, 157)}...` : text;
  }

  // Focus only ever moves the tab selection: no navigation, no attach, and no
  // creation. Requests are serialized with latest-wins coalescing, so a rapid
  // burst never stacks up: at most one tabs.focus is in flight and everything
  // that arrives meanwhile collapses to the newest target. Each target is
  // revalidated against the present when it runs, so a stale request cannot win.
  function handleFocus(raw) {
    if (disposed || !owned || !focusEnabled || focusBroken) return;
    const request = bridgeFocusRequest(raw, owner, Date.now());
    if (!request) return;
    if (recentFocus.has(request.requestID)) return;
    rememberFocus(request.requestID);
    pendingFocus = { raw, sessionID: request.sessionID };
    void drainFocus().catch((error) => diagnose(`focus drain stopped: ${describeError(error)}`));
  }

  async function drainFocus() {
    if (focusDraining) return;
    focusDraining = true;
    let stallClosed = false;
    try {
      while (!disposed && pendingFocus) {
        const request = pendingFocus;
        pendingFocus = undefined;
        if (!owned || !focusEnabled || focusBroken) continue;
        if (!bridgeFocusRequest(request.raw, owner, Date.now())) {
          diagnose(`dropped an expired focus request (${request.sessionID})`);
          continue;
        }
        // The membership read is awaited, so everything is re-checked against
        // the present before the focus actually happens.
        const snapshot = bridgeTabsSnapshot(await readTabs(), owner);
        if (disposed) return;
        if (!owned || !focusEnabled || focusBroken) continue;
        if (!bridgeFocusRequest(request.raw, owner, Date.now())) continue;
        if (!snapshot) continue;
        if (!snapshot.tabs.some((tab) => tab.sessionID === request.sessionID)) {
          diagnose(`dropped a focus request for a closed tab (${request.sessionID})`);
          continue;
        }
        // A newer target is already waiting: this one is stale by definition.
        if (pendingFocus) continue;
        // The host call is awaited to settlement, so a next focus can never
        // overlap a still-running one. The watchdog only reports a call that
        // never settles: it never cancels it or frees the queue for another.
        const focusCall = Promise.resolve().then(() => tabs.focus(request.sessionID));
        let watchdog;
        const stalled = new Promise((resolve) => {
          watchdog = setTimeout(() => resolve(BRIDGE_FOCUS_STALLED), BRIDGE_FOCUS_WATCHDOG_MS);
          watchdog.unref?.();
        });
        let result;
        try {
          result = await Promise.race([focusCall, stalled]);
        } catch (error) {
          clearTimeout(watchdog);
          if (!disposed) diagnose(`focus failed (${request.sessionID}): ${describeError(error)}`);
          continue;
        }
        clearTimeout(watchdog);
        if (result === BRIDGE_FOCUS_STALLED) {
          if (disposed) return;
          // The call is still outstanding and cannot be cancelled safely, so no
          // other focus may be launched over it and nothing queued is accepted.
          // Focus stays closed from here on: a late settlement is not treated
          // as proof that the surface is usable again (reload required).
          stallClosed = true;
          focusBroken = true;
          focusEnabled = false;
          pendingFocus = undefined;
          diagnose(`focus did not settle (${request.sessionID}); reload required`);
          // Reopen the drain only when the real call finally settles, and run
          // nothing afterwards. Both outcomes are handled, so a late rejection
          // cannot surface as an unhandled rejection.
          focusCall.then(
            () => {},
            () => {},
          ).then(() => {
            focusDraining = false;
          });
          return;
        }
        if (result === false) {
          diagnose(`focus was rejected (${request.sessionID})`);
          continue;
        }
        await refreshAfterFocus(request.sessionID);
      }
    } finally {
      if (!stallClosed) focusDraining = false;
    }
  }

  // A settled focus changes the active tab, so the surface must be republished
  // now instead of at the next poll. Bumping the epoch also invalidates any tab
  // list read that started earlier, so a stale response cannot overwrite it.
  async function refreshAfterFocus(sessionID) {
    focusEpoch += 1;
    const readback = await call("pane.current", { caller_pane_id: paneId });
    if (disposed) return;
    if (!readback || !readback.pane) {
      // A failed read says nothing about ownership. Keep what this pane already
      // verified, report the gap, and recover through a fresh bounded poll
      // instead of giving the surface up for the whole token lifetime.
      diagnose(`could not verify the tab surface after focusing (${sessionID})`);
      void poll();
      return;
    }
    const value = readback.pane.tokens?.[BRIDGE_TABS_TOKEN];
    const parsed = bridgeSnapshotOwner(value);
    if (!parsed || parsed.owner !== owner || parsed.empty) {
      // Someone else owns the surface now: stop focusing instead of pulling the
      // pane back to a tab it no longer shows.
      owned = false;
      focusEnabled = false;
      diagnose(`lost the tab surface while focusing (${sessionID})`);
      return;
    }
    const snapshot = bridgeTabsSnapshot(await readTabs(), owner, liveFlags);
    if (disposed) return;
    if (!snapshot) {
      diagnose(`could not refresh tabs after focusing (${sessionID})`);
      return;
    }
    // The host is authoritative: if it still reports another active tab the
    // focus did not take effect, and that must be visible. The snapshot is
    // still published so the surface recovers from what is really open.
    const active = snapshot.tabs.find((tab) => tab.active);
    if (!active || active.sessionID !== sessionID) {
      diagnose(`focus did not become the active tab (${sessionID})`);
    }
    lastValidated = { payload: snapshot.payload, tabs: snapshot.tabs };
    lastValidatedAt = Date.now();
    queuePublish(snapshot, true);
    // The authoritative snapshot is queued now. Any list read that started
    // before this point, including one that began during the readback above,
    // is stale and must not replace it.
    focusEpoch += 1;
  }

  function observePane(pane) {
    if (disposed || !pane || pane.pane_id !== paneId) return;
    const tokens = pane.tokens && typeof pane.tokens === "object" ? pane.tokens : {};
    const tabsValue = tokens[BRIDGE_TABS_TOKEN];
    const parsed = bridgeSnapshotOwner(tabsValue);
    const ownsLiveSnapshot = typeof tabsValue === "string" &&
      !!parsed &&
      parsed.owner === owner &&
      !parsed.empty;
    if (!ownsLiveSnapshot) {
      // No live snapshot under this pane's current owner: focus control has to
      // be re-earned through a fresh readback before it can be used again.
      owned = false;
      focusEnabled = false;
    }
    const focusValue = tokens[BRIDGE_FOCUS_TOKEN];
    if (focusValue === lastFocusRaw) return;
    lastFocusRaw = focusValue;
    if (typeof focusValue === "string" && focusValue) void handleFocus(focusValue);
  }

  // Exact caller pane only: reconciliation repairs dropped events, it is not a
  // discovery mechanism.
  async function observe() {
    if (disposed || observing) return;
    observing = true;
    try {
      const result = await call("pane.current", { caller_pane_id: paneId });
      if (!disposed && result?.pane) observePane(result.pane);
    } finally {
      observing = false;
    }
  }

  function scheduleReconnect(socket) {
    if (disposed) return;
    try {
      socket.destroy();
    } catch {}
    clearTimeout(eventTimer);
    eventTimer = setTimeout(connectEvents, backoff);
    eventTimer.unref?.();
    backoff = Math.min(backoff * 2, BRIDGE_BACKOFF_MAX_MS);
  }

  function connectEvents() {
    if (disposed) return;
    const subscribeId = `${BRIDGE_SOURCE}:${Date.now()}:${++sequence}`;
    let acknowledged = false;
    let ackTimer;
    const settleStream = () => {
      acknowledged = true;
      clearTimeout(ackTimer);
    };
    const socket = net.createConnection(endpoint, () => {
      if (disposed) {
        try {
          socket.destroy();
        } catch {}
        return;
      }
      backoff = BRIDGE_BACKOFF_MS;
      socket.write(`${JSON.stringify({
        id: subscribeId,
        method: "events.subscribe",
        params: {
          pane_id: paneId,
          source: BRIDGE_SOURCE,
          agent: AGENT,
          subscriptions: [{ type: "pane.updated" }],
        },
      })}\n`);
    });
    eventSocket = socket;
    eventBuffer = "";
    socket.on("data", (chunk) => {
      if (disposed) return;
      eventBuffer += chunk;
      if (eventBuffer.length > BRIDGE_MAX_LINE_BYTES) {
        eventBuffer = "";
        return;
      }
      let index;
      while ((index = eventBuffer.indexOf("\n")) !== -1) {
        const raw = eventBuffer.slice(0, index);
        eventBuffer = eventBuffer.slice(index + 1);
        let message;
        try {
          message = JSON.parse(raw);
        } catch {
          continue;
        }
        if (message?.id === subscribeId) {
          // A rejected or unshaped subscription acknowledgement is a broken
          // stream: drop it and retry with backoff instead of keeping a socket
          // that will never deliver events.
          if (!bridgeSuccessResult(message)) {
            scheduleReconnect(socket);
            return;
          }
          settleStream();
          continue;
        }
        const pane = message?.data?.pane ?? message?.pane;
        if (pane && typeof pane === "object") {
          settleStream();
          observePane(pane);
        }
      }
    });
    // Bounded wait for the acknowledgement: an unanswered subscribe is treated
    // as broken too, rather than left as a silently idle stream.
    ackTimer = setTimeout(() => {
      if (!acknowledged && !disposed) scheduleReconnect(socket);
    }, BRIDGE_SUBSCRIBE_ACK_TIMEOUT_MS);
    ackTimer.unref?.();
    socket.on("error", () => scheduleReconnect(socket));
    socket.on("end", () => scheduleReconnect(socket));
    socket.on("close", () => scheduleReconnect(socket));
  }

  const pollTimer = setInterval(() => {
    void poll();
    heartbeat();
  }, BRIDGE_POLL_INTERVAL_MS);
  pollTimer.unref?.();
  const reconcileTimer = setInterval(() => void observe(), BRIDGE_RECONCILE_INTERVAL_MS);
  reconcileTimer.unref?.();
  connectEvents();
  // Publish this pane's first snapshot immediately instead of after a full poll.
  void poll();
  void observe();

  // Best effort, and owner-gated: unloading the hook gives the tab surface back
  // to the pane immediately instead of waiting out the token TTL.
  function releaseOwnership() {
    if (!owned) return;
    owned = false;
    const snapshot = bridgeTabsSnapshot([], owner);
    if (!snapshot) return;
    const line = `${JSON.stringify({
      id: `${BRIDGE_SOURCE}:${Date.now()}:${++sequence}`,
      method: "pane.report_metadata",
      params: {
        pane_id: paneId,
        source: BRIDGE_SOURCE,
        agent: AGENT,
        tokens: { [BRIDGE_TABS_TOKEN]: snapshot.payload },
        seq: ++sequence,
        ttl_ms: BRIDGE_TABS_TTL_MS,
      },
    })}\n`;
    try {
      const socket = net.createConnection(endpoint, () => {
        try {
          socket.write(line);
        } catch {}
      });
      const timer = setTimeout(() => {
        try {
          socket.destroy();
        } catch {}
      }, BRIDGE_SOCKET_TIMEOUT_MS);
      timer.unref?.();
      socket.on("error", () => {});
      socket.on("close", () => clearTimeout(timer));
    } catch {}
  }

  return {
    dispose() {
      if (disposed) return;
      clearInterval(pollTimer);
      clearInterval(reconcileTimer);
      clearTimeout(eventTimer);
      try {
        eventSocket?.destroy();
      } catch {}
      eventBuffer = "";
      recentFocus.clear();
      pending?.dispose?.();
      releaseOwnership();
      disposed = true;
    },
  };
}
