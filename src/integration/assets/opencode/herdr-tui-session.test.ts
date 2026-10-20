import { afterEach, beforeEach, expect, mock, test } from "bun:test";

const requests: unknown[] = [];
const activeDisposers: Array<() => void> = [];
const requestWaiters: Array<() => void> = [];
const stateWaiters: Array<() => void> = [];
let importCounter = 0;
let holdConnections = false;
let failConnections = false;
const connections: Array<() => void> = [];

// Backend emulation for the tab bridge: `pane.report_metadata` stores tokens,
// `pane.current` reads them back, and `events.subscribe` opens a stream.
const paneTokens = new Map<string, string>();
const eventStreams: Array<(line: string) => void> = [];
let holdMetadata = false;
let failMetadata = false;
// Number of `pane.current` calls to answer with a transport-level error.
let paneCurrentFailures = 0;
let malformedReply = false;
let rejectSubscribe = false;
let silentSubscribe = false;
let tokenRewrite: (value: string, key: string) => string = (value) => value;

mock.module("node:net", () => ({
  default: {
    createConnection(_path: string, onConnect: () => void) {
      const handlers = new Map<string, (chunk?: unknown) => void>();
      const client = {
        destroyed: false,
        write(input: string) {
          if (client.destroyed) return;
          const request = JSON.parse(input.trim());
          requests.push(request);
          const id = isRecord(request) ? request.id : undefined;
          const method = isRecord(request) ? request.method : undefined;
          const reply = (result: unknown) => {
            if (malformedReply) {
              queueMicrotask(() => client.emit("data", `${JSON.stringify({ id, ok: true })}\n`));
              return;
            }
            queueMicrotask(() => client.emit("data", `${JSON.stringify({ id, result })}\n`));
          };
          if (method === "events.subscribe") {
            if (silentSubscribe) return;
            const line = rejectSubscribe
              ? { id, error: { code: "subscription_rejected", message: "pane.updated unavailable" } }
              : { id, result: { type: "subscription_started" } };
            queueMicrotask(() => client.emit("data", `${JSON.stringify(line)}\n`));
            if (!rejectSubscribe) {
              queueMicrotask(() => {
                if (!client.destroyed) eventStreams.push((streamLine) => client.emit("data", streamLine));
              });
            }
            return;
          }
          if (method === "pane.current") {
            if (paneCurrentFailures > 0) {
              paneCurrentFailures -= 1;
              queueMicrotask(() =>
                client.emit(
                  "data",
                  `${JSON.stringify({ id, error: { code: "unavailable", message: "transient read failure" } })}\n`,
                ),
              );
              return;
            }
            reply({
              type: "pane_current",
              pane: {
                pane_id: process.env.HERDR_PANE_ID,
                tokens: Object.fromEntries(paneTokens),
              },
            });
            return;
          }
          if (method === "pane.report_metadata") {
            if (holdMetadata) return;
            const tokens = isRecord(request.params) && isRecord(request.params.tokens)
              ? request.params.tokens
              : {};
            if (!failMetadata && !malformedReply) {
              for (const [key, value] of Object.entries(tokens)) {
                if (typeof value === "string") paneTokens.set(key, tokenRewrite(value, key));
              }
            }
            reply({ type: "ok" });
            return;
          }
          if (isRecord(request.params) && request.params.state !== undefined) {
            stateWaiters.shift()?.();
          }
          requestWaiters.shift()?.();
          reply({ type: "ok" });
        },
        setTimeout() {},
        on(event: string, handler: (chunk?: unknown) => void) {
          handlers.set(event, handler);
        },
        destroy() {
          client.destroyed = true;
        },
        emit(event: string, arg?: unknown) {
          if (event === "data") handlers.get(event)?.(arg);
          else handlers.get(event)?.();
        },
      };
      if (holdConnections) connections.push(onConnect);
      else if (failConnections) queueMicrotask(() => client.emit("error"));
      else queueMicrotask(onConnect);
      return client;
    },
  },
}));

beforeEach(() => {
  requests.length = 0;
  requestWaiters.length = 0;
  stateWaiters.length = 0;
  holdConnections = false;
  failConnections = false;
  connections.length = 0;
  paneTokens.clear();
  eventStreams.length = 0;
  holdMetadata = false;
  failMetadata = false;
  malformedReply = false;
  rejectSubscribe = false;
  silentSubscribe = false;
  tokenRewrite = (value) => value;
  paneCurrentFailures = 0;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = "test.sock";
  process.env.HERDR_PANE_ID = "test:p1";
});

afterEach(() => {
  for (const dispose of activeDisposers.splice(0)) {
    dispose();
  }
});

async function loadPlugin() {
  importCounter += 1;
  const module = await import(`./herdr-tui-session.js?test=${importCounter}`);
  return module.default;
}

type TabsStub = {
  list: () => unknown;
  focus: (sessionID: string) => Promise<void> | void;
};

function fakeApi(options: { tabs?: TabsStub; contextTabs?: TabsStub } = {}) {
  const sessions = new Map<string, { id: string; parentID?: string }>();
  const statuses: Record<string, { type: string }> = {};
  const permissions: Array<{ id: string; sessionID: string; tool?: { messageID: string; callID: string } }> = [];
  const questions: typeof permissions = [];
  const messages = new Map<string, { info?: { error?: { name: string } }; parts: Array<object> }>();
  const listeners = new Map<string, Set<(event: object) => void>>();
  const calls: string[] = [];
  let current: { name: string; params?: { sessionID: string } } = { name: "home" };
  let dispose: (() => void) | undefined;
  activeDisposers.push(() => dispose?.());

  return {
    statuses, permissions, questions, messages, listeners, calls,
    emit(type: string, properties: object) {
      for (const receive of listeners.get(type) ?? []) receive({ type, properties });
    },
    api: {
      client: {
        session: {
          async get({ sessionID }: { sessionID: string }) {
            calls.push(`get:${sessionID}`);
            const data = sessions.get(sessionID);
            if (!data) throw new Error("session not found");
            return { data };
          },
          async status() { calls.push("status"); return { data: { ...statuses } }; },
          async message({ messageID }: { sessionID: string; messageID: string }) {
            calls.push(`message:${messageID}`);
            const data = messages.get(messageID);
            if (!data) throw new Error("message unavailable");
            return { data };
          },
        },
        permission: { async list() { return { data: [...permissions] }; } },
        question: { async list() { return { data: [...questions] }; } },
      },
      event: {
        on(type: string, receive: (event: object) => void) {
          if (!listeners.has(type)) listeners.set(type, new Set());
          listeners.get(type)!.add(receive);
          return () => listeners.get(type)!.delete(receive);
        },
      },
      route: {
        get current() {
          return current;
        },
      },
      ui: { tabs: options.tabs },
      context: options.contextTabs ? { ui: { tabs: options.contextTabs } } : undefined,
      state: {
        session: {
          get(sessionID: string) {
            return sessions.get(sessionID);
          },
        },
      },
      lifecycle: {
        onDispose(handler: () => void) {
          dispose = handler;
          return () => {};
        },
      },
    },
    addSession(session: { id: string; parentID?: string }) {
      sessions.set(session.id, session);
    },
    select(sessionID: string) {
      current = { name: "session", params: { sessionID } };
    },
    home() { current = { name: "home" }; },
    dispose() {
      dispose?.();
    },
  };
}

function waitForNextRequest(): Promise<void> {
  return new Promise((resolve) => requestWaiters.push(resolve));
}

function waitForStateReport(): Promise<void> {
  return new Promise((resolve) => stateWaiters.push(resolve));
}

test("reports a root session when only the local route changes", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  await plugin.tui(tui.api);

  const dispatched = waitForNextRequest();
  tui.select("session-a");
  await dispatched;

  expect(requests).toHaveLength(1);
  expect(requestParam(requests[0], "agent_session_id")).toBe("session-a");
  expect(requestParam(requests[0], "session_start_source")).toBe("select");
  expect(requestParam(requests[0], "seq")).toBeUndefined();
});

test("retries an initial selection while Herdr detects the process", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  tui.select("session-a");

  await plugin.tui(tui.api);
  await new Promise((resolve) => setTimeout(resolve, 125));

  const selections = requests.filter((request) => requestParam(request, "state") === undefined);
  expect(selections.length).toBeGreaterThanOrEqual(2);
  expect(selections.every((request) => requestParam(request, "agent_session_id") === "session-a")).toBe(true);
});

test("does not report root sessions not selected by this TUI", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  tui.addSession({ id: "session-b" });
  tui.select("session-a");
  await plugin.tui(tui.api);

  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every((request) => requestParam(request, "agent_session_id") === "session-a")).toBe(
    true,
  );
});

test("does not replace the root session with a selected child session", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "root-session" });
  tui.addSession({ id: "child-session", parentID: "root-session" });
  tui.select("root-session");
  await plugin.tui(tui.api);
  await flushReports();

  tui.select("child-session");
  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "root-session")).toBe(true);
});

test("stops route polling when the TUI plugin is disposed", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "session-a" });
  await plugin.tui(tui.api);
  tui.dispose();
  tui.select("session-a");

  await new Promise((resolve) => setTimeout(resolve, 125));

  expect(requests).toHaveLength(0);
});

function requestParam(request: unknown, name: string): unknown {
  if (!isRecord(request) || !isRecord(request.params)) {
    return undefined;
  }
  return request.params[name];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function v2Api() {
  const sessions = new Map([
    ["a", { id: "a" }],
    ["b", { id: "b" }],
    ["child", { id: "child", parentID: "a" }],
    ["child2", { id: "child2", parentID: "a" }],
  ]);
  let route = { type: "session", sessionID: "a" };
  const listeners = new Set<(event: unknown) => void>();
  const permissions = new Map<string, Array<{ id: string }> | undefined>();
  const forms = new Map<string, Array<{ id: string }> | undefined>();
  const statuses = new Map<string, string>();
  return {
    api: {
      ui: { router: { current: () => route } },
      data: {
        session: {
          get: (id: string) => sessions.get(id),
          // Mirror the SDK: family returns the session's descendants, not every
          // cached session.
          family: (id: string) => {
            const result: string[] = [];
            for (const [memberID, session] of sessions) {
              if (memberID === id) continue;
              let cursor = session.parentID;
              while (cursor) {
                if (cursor === id) { result.push(memberID); break; }
                cursor = sessions.get(cursor)?.parentID;
              }
            }
            return result;
          },
          status: (id: string) => statuses.get(id) ?? "idle",
          permission: { list: (id: string) => permissions.get(id) },
          form: { list: (id: string) => forms.get(id) },
        },
        listen: (handler: (event: unknown) => void) => {
          listeners.add(handler);
          return () => listeners.delete(handler);
        },
      },
    },
    select(sessionID: string) { route = { type: "session", sessionID }; },
    home() { route = { type: "home", sessionID: "" }; },
    emit(type: string, data?: object) {
      for (const listener of listeners) listener({ details: { type, data } });
    },
    listeners,
    sessions,
    permissions,
    forms,
    statuses,
  };
}

const flushReports = () => new Promise((resolve) => setTimeout(resolve, 10));
const states = () => requests.filter((r) => requestParam(r, "state") !== undefined)
  .map((r) => requestParam(r, "state"));

function familyApi() {
  const tui = fakeApi();
  tui.addSession({ id: "root" });
  tui.addSession({ id: "child", parentID: "root" });
  tui.addSession({ id: "sibling", parentID: "root" });
  tui.addSession({ id: "grandchild", parentID: "child" });
  tui.addSession({ id: "other" });
  tui.select("root");
  return tui;
}

test("V1 hydrates active descendants and keeps working until the whole family settles", async () => {
  const tui = familyApi();
  tui.statuses.child = { type: "busy" };
  tui.statuses.grandchild = { type: "retry" };
  tui.statuses.other = { type: "busy" };
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.status", { sessionID: "child", status: { type: "idle" } });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.status", { sessionID: "grandchild", status: { type: "idle" } });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "root")).toBe(true);
});

test("V1 retains sibling blockers and cancellation does not finish an active parent", async () => {
  const tui = familyApi();
  tui.statuses.root = { type: "busy" };
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  tui.emit("permission.asked", { id: "p", sessionID: "child" });
  tui.emit("question.asked", { id: "q", sessionID: "sibling", tool: { messageID: "m", callID: "call" } });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { requestID: "p", sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("session.idle", { sessionID: "sibling" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("message.part.updated", { part: {
    type: "tool", sessionID: "sibling", messageID: "m", callID: "call", state: { status: "error" },
  } });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.idle", { sessionID: "root" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 hydration rejects aborted tool requests even if their session is busy again", async () => {
  for (const kind of ["permissions", "questions"] as const) {
    const tui = familyApi();
    tui.statuses.child = { type: "busy" };
    tui[kind].push({ id: "stale", sessionID: "child", tool: { messageID: "m", callID: "call" } });
    tui.messages.set("m", { info: { error: { name: "MessageAbortedError" } }, parts: [
      { type: "tool", callID: "call", state: { status: "error" } },
    ] });
    await (await loadPlugin()).tui(tui.api);
    await flushReports();
    expect(states().at(-1)).toBe("working");
    tui.dispose();
  }
});

test("V1 validates pending tools instead of assuming an idle owner has no requests", async () => {
  const tui = familyApi();
  tui.permissions.push({ id: "pending", sessionID: "child", tool: { messageID: "m", callID: "call" } });
  tui.messages.set("m", { parts: [{ type: "tool", callID: "call", state: { status: "running" } }] });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("message.part.updated", { part: {
    type: "tool", sessionID: "child", messageID: "m", callID: "call", state: { status: "error" },
  } });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 replays completion and replies over a late initial snapshot", async () => {
  const tui = familyApi();
  let resolveStatus!: (result: { data: Record<string, { type: string }> }) => void;
  tui.api.client.session.status = () => new Promise((resolve) => { resolveStatus = resolve; });
  tui.permissions.push({ id: "p", sessionID: "child" });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states()).not.toContain("idle");
  tui.emit("session.idle", { sessionID: "child" });
  tui.emit("permission.replied", { sessionID: "child", requestID: "p" });
  resolveStatus({ data: { child: { type: "busy" } } });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 retains replies received between failed hydration and its retry", async () => {
  const tui = familyApi();
  tui.permissions.push({ id: "p", sessionID: "child" });
  const status = tui.api.client.session.status;
  tui.api.client.session.status = async () => {
    tui.api.client.session.status = status;
    throw new Error("temporarily unavailable");
  };
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  tui.emit("permission.replied", { sessionID: "child", requestID: "p" });
  await new Promise((resolve) => setTimeout(resolve, 650));
  expect(states().at(-1)).toBe("idle");
  expect(states()).not.toContain("blocked");
});

test("V1 deleted sessions cannot return through a late hydration snapshot", async () => {
  const tui = familyApi();
  let resolveStatus!: (result: { data: Record<string, { type: string }> }) => void;
  tui.api.client.session.status = () => new Promise((resolve) => { resolveStatus = resolve; });
  tui.permissions.push({ id: "p", sessionID: "child" });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  tui.emit("session.deleted", { info: { id: "child", parentID: "root" } });
  resolveStatus({ data: { child: { type: "busy" } } });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 ignores stale hydration across A/B/A route changes and disposal", async () => {
  const tui = familyApi();
  let resolveOld!: (result: { data: Record<string, { type: string }> }) => void;
  const status = tui.api.client.session.status;
  tui.api.client.session.status = () => {
    tui.api.client.session.status = status;
    return new Promise((resolve) => { resolveOld = resolve; });
  };
  await (await loadPlugin()).tui(tui.api);
  tui.select("other");
  tui.emit("session.updated", { info: { id: "other" } });
  await flushReports();
  tui.select("root");
  tui.emit("session.updated", { info: { id: "root" } });
  await flushReports();
  requests.length = 0;
  resolveOld({ data: { root: { type: "busy" } } });
  await flushReports();
  expect(states()).not.toContain("working");
  tui.dispose();
  expect([...tui.listeners.values()].every((set) => set.size === 0)).toBe(true);
});

test("V1 a directly attached child owns its descendants, not its parent or siblings", async () => {
  const tui = familyApi();
  tui.select("child");
  tui.statuses.root = { type: "busy" };
  tui.statuses.sibling = { type: "busy" };
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  tui.emit("session.status", { sessionID: "grandchild", status: { type: "busy" } });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.select("grandchild");
  tui.emit("session.updated", { info: { id: "grandchild", parentID: "child" } });
  await flushReports();
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "child")).toBe(true);
});

test("V1 a terminal tool event during hydration cannot revive a cancelled blocker", async () => {
  const tui = familyApi();
  tui.questions.push({ id: "q", sessionID: "child", tool: { messageID: "m", callID: "call" } });
  let resolveMessage!: (result: { data: { parts: Array<object> } }) => void;
  tui.api.client.session.message = () => new Promise((resolve) => { resolveMessage = resolve; });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  tui.emit("message.part.updated", { part: {
    type: "tool", sessionID: "child", messageID: "m", callID: "call", state: { status: "error" },
  } });
  resolveMessage({ data: { parts: [{ type: "tool", callID: "call", state: { status: "running" } }] } });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 unknown tool evidence remains blocked and retries failed message reads", async () => {
  const tui = familyApi();
  tui.permissions.push({ id: "p", sessionID: "child", tool: { messageID: "m", callID: "call" } });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.messages.set("m", { parts: [{ type: "tool", callID: "call", state: { status: "error" } }] });
  await new Promise((resolve) => setTimeout(resolve, 650));
  expect(states().at(-1)).toBe("idle");
});

test("V1 request validation matches the exact tool and deduplicates message reads", async () => {
  const tui = familyApi();
  tui.permissions.push({ id: "p", sessionID: "child", tool: { messageID: "m", callID: "running" } });
  tui.questions.push({ id: "q", sessionID: "child", tool: { messageID: "m", callID: "finished" } });
  tui.messages.set("m", { parts: [
    { type: "tool", callID: "running", state: { status: "running" } },
    { type: "tool", callID: "finished", state: { status: "completed" } },
  ] });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(tui.calls.filter((call) => call === "message:m")).toHaveLength(1);
  tui.emit("permission.replied", { sessionID: "child", requestID: "p" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V1 reselecting an aborted request does not resurrect it or poll completed tools", async () => {
  const tui = familyApi();
  tui.permissions.push({ id: "p", sessionID: "child", tool: { messageID: "m", callID: "call" } });
  tui.messages.set("m", { parts: [{ type: "tool", callID: "call", state: { status: "error" } }] });
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  tui.select("other");
  tui.emit("session.updated", { info: { id: "other" } });
  await flushReports();
  tui.select("root");
  tui.emit("session.updated", { info: { id: "root" } });
  await flushReports();
  expect(states()).not.toContain("blocked");
  const reads = tui.calls.length;
  await new Promise((resolve) => setTimeout(resolve, 650));
  expect(tui.calls).toHaveLength(reads);
});

test("V1 home and selected-session deletion settle authority and retry a dropped idle report", async () => {
  for (const action of ["home", "delete"]) {
    const tui = familyApi();
    tui.statuses.root = { type: "busy" };
    await (await loadPlugin()).tui(tui.api);
    await flushReports();
    expect(states().at(-1)).toBe("working");
    failConnections = true;
    if (action === "home") {
      tui.home();
      tui.emit("session.updated", { info: { id: "root" } });
    } else tui.emit("session.deleted", { info: { id: "root" } });
    await flushReports();
    failConnections = false;
    await new Promise((resolve) => setTimeout(resolve, 650));
    expect(states().at(-1)).toBe("idle");
    tui.dispose();
  }
});

test("V1 a delayed home settlement cannot overwrite the next selected session", async () => {
  const tui = familyApi();
  tui.statuses.root = { type: "busy" };
  await (await loadPlugin()).tui(tui.api);
  await flushReports();
  requests.length = 0;
  holdConnections = true;
  tui.home();
  tui.emit("session.updated", { info: { id: "root" } });
  await flushReports();
  expect(connections.length).toBeGreaterThan(0);
  tui.select("other");
  tui.emit("session.updated", { info: { id: "other" } });
  holdConnections = false;
  for (const connect of connections.splice(0)) connect();
  await flushReports();
  expect(requests.length).toBeGreaterThan(0);
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "other")).toBe(true);
  expect(states().at(-1)).toBe("idle");
});

test("V2 ignores events without data", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  expect(() => tui.emit("legacy.event")).not.toThrow();
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  expect(states()).toEqual(["working"]);
});

test("V2 completes, interrupts, and fails without legacy idle events", async () => {
  for (const terminal of ["succeeded", "interrupted", "failed"]) {
    const plugin = await loadPlugin();
    const tui = v2Api();
    const dispose = await plugin.setup(tui.api);
    activeDisposers.push(dispose);
    await flushReports();
    requests.length = 0;
    tui.emit("session.execution.started", { sessionID: "a" });
    tui.emit(`session.execution.${terminal}`, { sessionID: "a" });
    await flushReports();
    // A failure is not an input request, so it settles like a normal finish.
    expect(states()).toEqual(["working", "idle"]);
    dispose();
  }
});

test("a failed execution does not block the pane without a pending request", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;

  tui.emit("session.execution.failed", { sessionID: "a" });
  await flushReports();

  expect(states()).not.toContain("blocked");
  expect(states().at(-1)).toBe("idle");
});

test("V2 aggregates root and child blockers and ignores other roots and child completion", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("permission.asked", { sessionID: "a", id: "permission-a" });
  tui.emit("form.created", { form: { sessionID: "child", id: "form-child" } });
  tui.emit("permission.replied", { sessionID: "a", requestID: "permission-a" });
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  tui.emit("session.execution.started", { sessionID: "b" });
  tui.emit("permission.asked", { sessionID: "b", id: "other" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "a")).toBe(true);
  tui.emit("form.cancelled", { sessionID: "child", id: "form-child" });
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await flushReports();
  expect(states().slice(-2)).toEqual(["working", "idle"]);
});

test("V2 discards queued reports after selection changes and stops on disposal", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.select("b");
  tui.emit("session.execution.started", { sessionID: "b" });
  await flushReports();
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "b")).toBe(true);
  requests.length = 0;
  tui.emit("session.execution.succeeded", { sessionID: "b" });
  tui.home();
  await flushReports();
  expect(requests).toHaveLength(0);
  dispose();
  expect(tui.listeners.size).toBe(0);
  tui.select("a");
  await new Promise((resolve) => setTimeout(resolve, 250));
  expect(requests).toHaveLength(0);
});

test("V2 reconciles late blocker hydration without reviving an already-replied request", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  tui.permissions.set("child", [{ id: "late" }]);
  await waitForStateReport();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { sessionID: "child", requestID: "late" });
  await waitForStateReport();
  expect(states().at(-1)).toBe("idle");
  tui.permissions.set("child", []);
  tui.forms.set("child", [{ id: "second" }]);
  await waitForStateReport();
  expect(states().at(-1)).toBe("blocked");
  tui.sessions.delete("child");
  tui.emit("session.deleted", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
});

test("V2 never writes a delayed connection after disposal or a session switch", async () => {
  for (const action of ["dispose", "switch"]) {
    const plugin = await loadPlugin();
    const tui = v2Api();
    holdConnections = true;
    requests.length = 0;
    const dispose = await plugin.setup(tui.api);
    activeDisposers.push(dispose);
    await flushReports();
    expect(connections.length).toBeGreaterThan(0);
    if (action === "dispose") dispose();
    else tui.select("b");
    holdConnections = false;
    for (const connect of connections.splice(0)) connect();
    await flushReports();
    expect(requests).toHaveLength(0);
    dispose();
  }
});

test("V2 settles a connection that never completes", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  holdConnections = true;
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  const started = Date.now();
  while (connections.length <= 1 && Date.now() - started < 2_000) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  expect(connections.length).toBeGreaterThan(1);
  dispose();
});

test("V2 resends the latest state after a failed delivery", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  // Exhaust the selection retry schedule so only the event report remains.
  await new Promise((resolve) => setTimeout(resolve, 1_600));
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  failConnections = true;
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  const resend = waitForStateReport();
  await new Promise((resolve) => setTimeout(resolve, 700));
  failConnections = false;
  await resend;
  expect(states().at(-1)).toBe("idle");
  dispose();
});

test("V2 keeps the pane working while any descendant runs", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "a" });
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.emit("session.execution.started", { sessionID: "child2" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  // The selected session finishing must not erase its running children.
  tui.emit("session.execution.succeeded", { sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  // One child finishing must keep the pane working while the other still runs.
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.execution.succeeded", { sessionID: "child2" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "a")).toBe(true);
  dispose();
});

test("V2 hydrates already-running descendants when the pane attaches", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  tui.statuses.set("child", "running");
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.execution.succeeded", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  dispose();
});

test("V2 does not block input when a descendant fails without a request", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.emit("session.execution.failed", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  expect(states()).not.toContain("blocked");
  dispose();
});

test("V2 keeps blocker priority over running descendants", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.emit("permission.asked", { sessionID: "child", id: "permission-child" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");
  tui.emit("permission.replied", { sessionID: "child", requestID: "permission-child" });
  await flushReports();
  expect(states().at(-1)).toBe("working");
  dispose();
});

test("V2 selection switch stops counting the previous subtree's running children", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  tui.statuses.set("child", "running");
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.select("b");
  // Selection is poll/event driven; a routed event triggers the resync.
  tui.emit("session.updated", { sessionID: "b" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  requests.length = 0;
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.emit("session.execution.started", { sessionID: "a" });
  await flushReports();
  expect(states()).not.toContain("working");
  expect(requests.every((r) => requestParam(r, "agent_session_id") === "b")).toBe(true);
  dispose();
});

test("V2 deleting a running descendant clears its working contribution", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  tui.statuses.set("child", "running");
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  await flushReports();
  expect(states().at(-1)).toBe("working");
  tui.sessions.delete("child");
  tui.emit("session.deleted", { sessionID: "child" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  dispose();
});

// Tab bridge

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function bridgeApi(initial: unknown, options: { live?: Record<string, unknown> } = {}) {
  const tui = fakeApi();
  const focusCalls: string[] = [];
  let listValue: unknown = initial;
  const listOverrides: unknown[] = [];
  let listThrows = false;
  let focusThrows = false;
  let focusResult: unknown;
  let holdList = false;
  let holdFocus = false;
  let focusActivates = false;
  const listResolvers: Array<() => void> = [];
  const focusResolvers: Array<() => void> = [];
  let focusActive = 0;
  let focusPeak = 0;
  tui.api.ui.tabs = {
    list() {
      if (listThrows) throw new Error("tabs unavailable");
      const value = listOverrides.length > 0 ? listOverrides.shift() : listValue;
      if (!holdList) return value;
      return new Promise((resolve) => {
        listResolvers.push(() => resolve(value));
      });
    },
    async focus(sessionID: string) {
      focusCalls.push(sessionID);
      focusActive += 1;
      focusPeak = Math.max(focusPeak, focusActive);
      try {
        if (holdFocus) await new Promise<void>((resolve) => focusResolvers.push(resolve));
        if (focusThrows) throw new Error("focus rejected");
        if (focusActivates && Array.isArray(listValue)) {
          listValue = listValue.map((tab) =>
            isRecord(tab) && typeof tab.sessionID === "string"
              ? { ...tab, active: tab.sessionID === sessionID }
              : tab,
          );
        }
        return focusResult;
      } finally {
        focusActive -= 1;
      }
    },
  };
  if (options.live) (tui.api as Record<string, unknown>).data = options.live;
  return {
    tui,
    focusCalls,
    setTabs(value: unknown) { listValue = value; },
    enqueueList(value: unknown) { listOverrides.push(value); },
    setListThrows(value: boolean) { listThrows = value; },
    setFocusThrows(value: boolean) { focusThrows = value; },
    setFocusResult(value: unknown) { focusResult = value; },
    setFocusActivates(value: boolean) { focusActivates = value; },
    setHoldList(value: boolean) { holdList = value; },
    setHoldFocus(value: boolean) { holdFocus = value; },
    pendingLists() { return listResolvers.length; },
    releaseList(index = 0) { listResolvers.splice(index, 1)[0]?.(); },
    releaseLatestList() { listResolvers.pop()?.(); },
    releaseFocus(index = 0) { focusResolvers.splice(index, 1)[0]?.(); },
    peakFocus() { return focusPeak; },
    setMalformedReply(value: boolean) { malformedReply = value; },
    setRejectSubscribe(value: boolean) { rejectSubscribe = value; },
    setSilentSubscribe(value: boolean) { silentSubscribe = value; },
    setPaneCurrentFailures(value: number) { paneCurrentFailures = value; },
  };
}

const metadataRequests = () => requests.filter((r) => isRecord(r) && r.method === "pane.report_metadata");
const tabsTokenOf = (request: unknown) => {
  const params = isRecord(request) && isRecord(request.params) ? request.params : undefined;
  const tokens = isRecord(params) && isRecord(params.tokens) ? params.tokens : undefined;
  return isRecord(tokens) ? tokens["herdr_oc_tabs"] : undefined;
};
const lastTabsToken = () => tabsTokenOf(metadataRequests().at(-1));
const lastTabsPayload = () => {
  const value = lastTabsToken();
  return typeof value === "string" ? JSON.parse(value) : undefined;
};
const hexId = (n: number) => n.toString(16).padStart(32, "0");
const emitPane = (tokens: Record<string, unknown>) => {
  const line = `${JSON.stringify({
    event: "pane.updated",
    data: { type: "pane_updated", pane: { pane_id: process.env.HERDR_PANE_ID, tokens } },
  })}\n`;
  for (const push of [...eventStreams]) push(line);
};
const focusPayload = (overrides: Record<string, unknown> = {}) => JSON.stringify({
  v: 1,
  owner: lastTabsPayload()?.owner,
  sessionID: "root-a",
  requestID: hexId(1),
  expiresAtMs: Date.now() + 10_000,
  ...overrides,
});

async function waitFor(check: () => boolean, timeout = 3_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (check()) return true;
    await sleep(5);
  }
  return check();
}

const rootTab = (sessionID: string, overrides: Record<string, unknown> = {}) => ({
  sessionID, title: sessionID, active: false, busy: false, attention: false, unread: false, ...overrides,
});

test("tab bridge stays off when the host exposes no tabs API", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi();
  tui.addSession({ id: "root-a" });
  await plugin.tui(tui.api);

  const dispatched = waitForNextRequest();
  tui.select("root-a");
  await dispatched;
  await sleep(200);

  expect(metadataRequests()).toHaveLength(0);
  expect(requests.some((r) => isRecord(r) && r.method === "pane.current")).toBe(false);
  expect(requests.some((r) => isRecord(r) && r.method === "events.subscribe")).toBe(false);
  expect(requestParam(requests.at(-1), "agent_session_id")).toBe("root-a");
});

test("publishes a normalized snapshot and drops blank placeholder rows", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([
    rootTab("root-a", { title: "Root\u0007\u001bq A", active: true, busy: true, unread: true, attention: false }),
    rootTab("root-b", { title: 42, attention: true }),
    { sessionID: "", title: "blank" },
    { title: "no session id" },
    { sessionID: "   ", title: "New session" },
    { sessionID: "\t\n", title: "whitespace only", active: true },
  ]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);

  const payload = lastTabsPayload();
  expect(payload.v).toBe(1);
  expect(payload.owner).toMatch(/^[0-9a-f]{32}$/);
  expect(payload.tabs).toEqual([
    { sessionID: "root-a", title: "Rootq A", active: true, busy: true, attention: false, unread: true },
    { sessionID: "root-b", title: "", active: false, busy: false, attention: true, unread: false },
  ]);
  // Blank rows are excluded, never published, and never reject their siblings.
  expect(payload.tabs.every((tab: { sessionID: string }) => tab.sessionID.trim().length > 0)).toBe(true);
  expect(requestParam(metadataRequests().at(-1), "ttl_ms")).toBe(15000);
  expect(requestParam(metadataRequests().at(-1), "seq")).toBeGreaterThan(0);
});

test("rejects an untrustworthy tab list instead of publishing a partial one", async () => {
  const cases: unknown[] = [
    [rootTab("root-a"), rootTab("root-a")],
    [rootTab("root-a", { active: true }), rootTab("root-b", { active: true })],
    [rootTab("root a")],
    [rootTab(" root-a")],
    [rootTab("bad\u00e9id")],
    [rootTab("x".repeat(129))],
    Array.from({ length: 65 }, (_, index) => rootTab(`root-${index}`)),
  ];
  for (const list of cases) {
    const plugin = await loadPlugin();
    const bridge = bridgeApi(list);
    bridge.tui.addSession({ id: "root-a" });
    await plugin.tui(bridge.tui.api);
    await sleep(120);
    expect(metadataRequests()).toHaveLength(0);
    bridge.tui.dispose();
  }
});

test("normalizes titles to 128 Unicode scalars without control characters", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { title: `${"a".repeat(140)}`, active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);

  expect(lastTabsPayload().tabs[0].title).toBe("a".repeat(128));
});

test("focuses only the requested tab for a fresh, owned request from this pane", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  let focused = false;
  let accepted = "";
  for (let attempt = 0; attempt < 60 && !focused; attempt += 1) {
    accepted = hexId(0x100 + attempt);
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: accepted }) });
    await sleep(25);
    focused = bridge.focusCalls.length > 0;
  }
  expect(focused).toBe(true);
  expect(bridge.focusCalls).toEqual(["root-a"]);

  // Wrong owner, expired, future, and malformed requests are ignored.
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ owner: "f".repeat(32), requestID: hexId(0x200) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ expiresAtMs: Date.now() - 1, requestID: hexId(0x201) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ expiresAtMs: Date.now() + 10_000_000, requestID: hexId(0x202) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ expiresAtMs: "soon", requestID: hexId(0x203) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: "ABCDEF0123456789ABCDEF0123456789" }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: "not json" });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-z", requestID: hexId(0x205) }) });
  await sleep(150);
  expect(bridge.focusCalls).toEqual(["root-a"]);

  // The same request id is never replayed.
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: accepted }) });
  await sleep(100);
  expect(bridge.focusCalls).toEqual(["root-a"]);

  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x300) }) });
  await waitFor(() => bridge.focusCalls.length > 1, 1_000);
  expect(bridge.focusCalls).toEqual(["root-a", "root-b"]);
});

test("disables focus when the readback of this pane's token is truncated", async () => {
  tokenRewrite = (value) => value.slice(0, 80);
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());
  expect(token.length).toBeGreaterThan(80);

  for (let attempt = 0; attempt < 12; attempt += 1) {
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x400 + attempt) }) });
    await sleep(50);
  }
  expect(bridge.focusCalls).toEqual([]);
  expect(requests.some((r) => isRecord(r) && r.method === "pane.current")).toBe(true);
});

test("keeps focus working when a focus call itself fails", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setFocusThrows(true);
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  let attempts = 0;
  while (bridge.focusCalls.length === 0 && attempts < 60) {
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x500 + attempts) }) });
    await sleep(25);
    attempts += 1;
  }
  expect(bridge.focusCalls.length).toBeGreaterThan(0);

  bridge.setFocusThrows(false);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-a", requestID: hexId(0x600) }) });
  await sleep(100);
  expect(bridge.focusCalls.length).toBeGreaterThan(1);
});

test("ignores fragmented and malformed event lines without losing later events", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  const push = (chunk: string) => {
    for (const stream of [...eventStreams]) stream(chunk);
  };
  push("this is not json\n");
  push(`{"event":"pane.updated","data":{"pane":{"pane_id":"${process.env.HERDR_PANE_ID}",`);
  await sleep(50);
  push(`"tokens":{"herdr_oc_focus":${JSON.stringify(focusPayload({ requestID: hexId(0x700) }))},"herdr_oc_tabs":${JSON.stringify(token)}}}}}\n`);
  await sleep(50);
  push('{"broken":\n');
  await waitFor(() => bridge.focusCalls.length > 0, 1_000);
  expect(bridge.focusCalls).toEqual(["root-a"]);
});

test("survives a failing tabs API and recovers on the next poll", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setListThrows(true);
  await plugin.tui(bridge.tui.api);
  await sleep(150);
  expect(metadataRequests()).toHaveLength(0);

  bridge.setListThrows(false);
  await waitFor(() => metadataRequests().length > 0, 2_000);
  expect(lastTabsPayload().tabs).toHaveLength(1);
});

test("retries an unacknowledged report and returns to a bounded poll", async () => {
  holdMetadata = true;
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length >= 2, 2_500);
  expect(paneTokens.size).toBe(0);

  holdMetadata = false;
  await waitFor(() => paneTokens.has("herdr_oc_tabs"), 2_500);
  const attempts = metadataRequests().length;
  expect(attempts).toBeLessThanOrEqual(6);
}, 15_000);

test("republishes an unchanged snapshot on a heartbeat, not on every poll", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  const first = String(lastTabsToken());

  await sleep(1_200);
  expect(metadataRequests()).toHaveLength(1);

  await waitFor(() => metadataRequests().length >= 2, 7_000);
  const published = metadataRequests();
  expect(published.length).toBeLessThanOrEqual(4);
  expect(tabsTokenOf(published.at(-1))).toBe(first);
  const sequences = published.map((r) => requestParam(r, "seq") as number);
  expect(sequences.every((value, index) => index === 0 || value > (sequences[index - 1] as number))).toBe(true);
}, 15_000);

test("disposal releases the pane's tabs and stops polling", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => requests.some((r) => isRecord(r) && r.method === "pane.current"));
  await sleep(50);
  const owner = lastTabsPayload().owner;
  const before = requests.length;

  bridge.tui.dispose();
  await sleep(50);

  const released = lastTabsPayload();
  expect(released).toEqual({ v: 1, owner, tabs: [] });
  expect(requests.length).toBe(before + 1);

  for (const stream of [...eventStreams]) {
    stream(`${JSON.stringify({
      event: "pane.updated",
      data: { type: "pane_updated", pane: { pane_id: process.env.HERDR_PANE_ID, tokens: { herdr_oc_focus: focusPayload({ requestID: hexId(0x800) }) } } },
    })}\n`);
  }
  await sleep(1_200);
  expect(requests.length).toBe(before + 1);
  expect(bridge.focusCalls).toEqual([]);
});

test("disposal during an in-flight report neither throws nor keeps polling", async () => {
  holdMetadata = true;
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await sleep(20);
  bridge.tui.dispose();
  const after = requests.length;
  holdMetadata = false;
  await sleep(1_200);
  expect(requests.length).toBe(after);
});

test("publishes through the context.ui.tabs surface with the bridge wire contract", async () => {
  const plugin = await loadPlugin();
  const tui = fakeApi({
    contextTabs: {
      list: () => [rootTab("root-a", { active: true })],
      focus: async () => {},
    },
  });
  tui.addSession({ id: "root-a" });
  await plugin.tui(tui.api);
  await waitFor(() => metadataRequests().length > 0);

  const request = metadataRequests().at(-1);
  expect(requestParam(request, "source")).toBe("herdr:opencode:tabs");
  expect(requestParam(request, "agent")).toBe("opencode");
  expect(requestParam(request, "pane_id")).toBe("test:p1");
  expect(lastTabsPayload().tabs).toEqual([
    { sessionID: "root-a", title: "root-a", active: true, busy: false, attention: false, unread: false },
  ]);
});

test("stops refreshing a snapshot the tab list no longer confirms, then republishes it once the token expires", { timeout: 40_000 }, async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);

  const owner = lastTabsPayload().owner;
  expect(metadataRequests()).toHaveLength(1);

  // The tab list stops answering. Past the confirmation grace the heartbeat must
  // stop too, so the stored token is left to expire instead of keeping rows for
  // tabs that are gone.
  bridge.setTabs(undefined);
  bridge.setListThrows(true);
  await sleep(6_500);
  expect(metadataRequests()).toHaveLength(1);

  // Still broken past the 15s token lifetime: no refresh at all.
  await sleep(9_500);
  expect(metadataRequests()).toHaveLength(1);

  // The list recovers with an identical snapshot. The payload did not change, but
  // the stored token expired, so it has to be published again.
  bridge.setTabs([rootTab("root-a", { active: true })]);
  bridge.setListThrows(false);
  const recovered = await waitFor(() => metadataRequests().length > 1, 4_000);
  expect(recovered).toBe(true);
  expect(lastTabsPayload()).toEqual({
    v: 1,
    owner,
    tabs: [
      { sessionID: "root-a", title: "root-a", active: true, busy: false, attention: false, unread: false },
    ],
  });
});

test("will not focus once this pane no longer owns a live snapshot", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);

  const token = String(lastTabsToken());
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x11) }) });
  await waitFor(() => bridge.focusCalls.length > 0);
  expect(bridge.focusCalls).toEqual(["root-a"]);

  // Another owner has taken the surface: focus control is given up, so the next
  // request is ignored instead of stealing the pane back.
  emitPane({
    herdr_oc_tabs: JSON.stringify({
      v: 1,
      owner: "f".repeat(32),
      tabs: [{ sessionID: "root-b", title: "b", active: true, busy: false, attention: false, unread: false }],
    }),
  });
  await sleep(50);
  emitPane({ herdr_oc_focus: focusPayload({ requestID: hexId(0x12) }) });
  await sleep(250);
  expect(bridge.focusCalls).toEqual(["root-a"]);
});

test("re-checks the request expiry after the awaited tab list read", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);

  // The focus handler awaits the live tab list. By the time it resolves the
  // request has expired, so nothing may be focused.
  const token = String(lastTabsToken());
  let release!: (value: unknown) => void;
  bridge.setTabs(new Promise((resolve) => { release = resolve; }));
  emitPane({
    herdr_oc_tabs: token,
    herdr_oc_focus: focusPayload({ requestID: hexId(0x21), expiresAtMs: Date.now() + 120 }),
  });
  await sleep(300);
  release([rootTab("root-a", { active: true })]);
  await sleep(300);

  expect(bridge.focusCalls).toEqual([]);
});

test("treats a response that only echoes the request id as a failure", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setMalformedReply(true);
  await plugin.tui(bridge.tui.api);

  // The id-only line never counts as success, so the report is retried and the
  // token is never treated as published.
  await waitFor(() => metadataRequests().length > 1, 3_000);
  expect(metadataRequests().length).toBeGreaterThan(1);
  expect(paneTokens.has("herdr_oc_tabs")).toBe(false);

  // A well-formed success envelope is accepted again.
  bridge.setMalformedReply(false);
  const accepted = await waitFor(() => paneTokens.has("herdr_oc_tabs"), 3_000);
  expect(accepted).toBe(true);
  expect(paneTokens.get("herdr_oc_tabs")).toBe(lastTabsToken());
});

test("retries a rejected event subscription instead of keeping a dead stream", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setRejectSubscribe(true);
  await plugin.tui(bridge.tui.api);

  const subscribes = () =>
    requests.filter((request) => isRecord(request) && request.method === "events.subscribe").length;
  const retried = await waitFor(() => subscribes() >= 2, 3_000);
  expect(retried).toBe(true);
  expect(eventStreams).toHaveLength(0);

  bridge.setRejectSubscribe(false);
  const streamed = await waitFor(() => eventStreams.length > 0, 3_000);
  expect(streamed).toBe(true);
});

test("reconnects when the event subscription is never acknowledged", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true })]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setSilentSubscribe(true);
  await plugin.tui(bridge.tui.api);

  const subscribes = () =>
    requests.filter((request) => isRecord(request) && request.method === "events.subscribe").length;
  const retried = await waitFor(() => subscribes() >= 2, 3_000);
  expect(retried).toBe(true);
  expect(eventStreams).toHaveLength(0);
});

test("coalesces a rapid burst to the newest focus target without overlapping", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b"), rootTab("root-c")]);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  // Hold the membership read so the burst arrives while the first target is
  // still being validated.
  bridge.setHoldList(true);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-a", requestID: hexId(0x71) }) });
  await waitFor(() => bridge.pendingLists() > 0);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x72) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-c", requestID: hexId(0x73) }) });
  expect(bridge.focusCalls).toEqual([]);

  bridge.releaseLatestList();
  await waitFor(() => bridge.pendingLists() > 0, 2_000);
  bridge.releaseLatestList();
  await waitFor(() => bridge.focusCalls.length > 0);

  // Only the newest target runs: root-a was superseded, root-b was replaced.
  expect(bridge.focusCalls).toEqual(["root-c"]);
  expect(bridge.peakFocus()).toBe(1);
});

test("serializes focus calls, coalesces the waiting burst, and lands on the newest tab", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b"), rootTab("root-c")]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setHoldFocus(true);
  bridge.setFocusActivates(true);
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-a", requestID: hexId(0x81) }) });
  await waitFor(() => bridge.focusCalls.length === 1);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x82) }) });
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-c", requestID: hexId(0x83) }) });
  await sleep(100);

  // A second focus never overlaps the running one, and the burst collapsed.
  expect(bridge.focusCalls).toEqual(["root-a"]);
  expect(bridge.peakFocus()).toBe(1);

  bridge.releaseFocus();
  await waitFor(() => bridge.focusCalls.length > 1);
  expect(bridge.focusCalls).toEqual(["root-a", "root-c"]);
  bridge.releaseFocus();

  await waitFor(
    () => lastTabsPayload()?.tabs?.some((tab: Record<string, unknown>) => tab.sessionID === "root-c" && tab.active) === true,
    2_000,
  );
  expect(bridge.peakFocus()).toBe(1);
});

test("drops a queued focus request that expires while an earlier focus runs", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setHoldFocus(true);
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x91) }) });
  await waitFor(() => bridge.focusCalls.length === 1);
  emitPane({
    herdr_oc_tabs: token,
    herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x92), expiresAtMs: Date.now() + 120 }),
  });
  await sleep(200);
  bridge.releaseFocus();
  await sleep(250);

  expect(bridge.focusCalls).toEqual(["root-a"]);
});

test("stops focusing when another owner takes the surface while a request waits", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setHoldFocus(true);
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x93) }) });
  await waitFor(() => bridge.focusCalls.length === 1);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x94) }) });
  // The pane now reads back another owner's snapshot.
  paneTokens.set("herdr_oc_tabs", JSON.stringify({
    v: 1,
    owner: "f".repeat(32),
    tabs: [{ sessionID: "root-b", title: "b", active: true, busy: false, attention: false, unread: false }],
  }));
  bridge.releaseFocus();
  await sleep(300);

  expect(bridge.focusCalls).toEqual(["root-a"]);
});

test("reports focus failures instead of swallowing them", async () => {
  const warnings: string[] = [];
  const originalWarn = console.warn;
  console.warn = (...args: unknown[]) => {
    warnings.push(args.map(String).join(" "));
  };
  try {
    const plugin = await loadPlugin();
    const bridge = bridgeApi([rootTab("root-a", { active: true })]);
    bridge.tui.addSession({ id: "root-a" });
    bridge.setFocusResult(false);
    await plugin.tui(bridge.tui.api);
    await waitFor(() => metadataRequests().length > 0);
    await waitFor(() => eventStreams.length > 0);
    const token = String(lastTabsToken());

    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0xa1) }) });
    await waitFor(() => bridge.focusCalls.length === 1);
    const rejected = await waitFor(() => warnings.some((line) => line.includes("rejected")));
    expect(rejected).toBe(true);

    bridge.setFocusResult(undefined);
    bridge.setFocusThrows(true);
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0xa2) }) });
    await waitFor(() => bridge.focusCalls.length === 2);
    const thrown = await waitFor(() => warnings.some((line) => line.includes("focus failed")));
    expect(thrown).toBe(true);

    bridge.setFocusThrows(false);
    paneTokens.delete("herdr_oc_tabs");
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0xa3) }) });
    await waitFor(() => bridge.focusCalls.length === 3);
    const lost = await waitFor(() => warnings.some((line) => line.includes("lost the tab surface")));
    expect(lost).toBe(true);

    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0xa4) }) });
    await sleep(150);
    expect(bridge.focusCalls).toEqual(["root-a", "root-a", "root-a"]);
  } finally {
    console.warn = originalWarn;
  }
});

test("republishes immediately after a focus instead of waiting for the poll", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
  bridge.tui.addSession({ id: "root-a" });
  bridge.setFocusActivates(true);
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());
  const before = metadataRequests().length;

  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0xb1) }) });
  await waitFor(() => bridge.focusCalls.length === 1);

  // Well inside the one second poll interval.
  const refreshed = await waitFor(() => metadataRequests().length > before, 400);
  expect(refreshed).toBe(true);
  expect(
    lastTabsPayload().tabs.some((tab: Record<string, unknown>) => tab.sessionID === "root-b" && tab.active),
  ).toBe(true);
});

test("discards a tab list read that started before a focus settled", async () => {
  const oldList = [rootTab("root-a", { active: true }), rootTab("root-c")];
  const newList = [rootTab("root-a"), rootTab("root-c", { active: true })];
  const plugin = await loadPlugin();
  const bridge = bridgeApi(oldList);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  // A poll read captures the pre-focus list and stalls there.
  bridge.setHoldList(true);
  const stalled = await waitFor(() => bridge.pendingLists() > 0, 2_500);
  expect(stalled).toBe(true);

  bridge.setTabs(newList);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-c", requestID: hexId(0xc1) }) });
  await waitFor(() => bridge.pendingLists() > 1, 1_000);
  bridge.setHoldList(false);
  bridge.releaseLatestList();
  await waitFor(() => bridge.focusCalls.length === 1);
  await waitFor(
    () => lastTabsPayload()?.tabs?.some((tab: Record<string, unknown>) => tab.sessionID === "root-c" && tab.active) === true,
    1_000,
  );

  // Releasing the older read must not publish its stale active tab.
  bridge.releaseList(0);
  await sleep(150);
  expect(
    lastTabsPayload().tabs.some((tab: Record<string, unknown>) => tab.sessionID === "root-c" && tab.active),
  ).toBe(true);
});

test("reports a focus that settled without becoming the active tab", async () => {
  const warnings: string[] = [];
  const originalWarn = console.warn;
  console.warn = (...args: unknown[]) => {
    warnings.push(args.map(String).join(" "));
  };
  try {
    const plugin = await loadPlugin();
    const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
    bridge.tui.addSession({ id: "root-a" });
    await plugin.tui(bridge.tui.api);
    await waitFor(() => metadataRequests().length > 0);
    await waitFor(() => eventStreams.length > 0);
    const token = String(lastTabsToken());
    const before = metadataRequests().length;

    // The call reports success but the host keeps another tab active.
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0xd1) }) });
    await waitFor(() => bridge.focusCalls.length === 1);

    const reported = await waitFor(() => warnings.some((line) => line.includes("did not become the active tab")));
    expect(reported).toBe(true);
    // The authoritative snapshot is still published so the surface recovers.
    const refreshed = await waitFor(() => metadataRequests().length > before, 1_000);
    expect(refreshed).toBe(true);
    expect(
      lastTabsPayload().tabs.find((tab: Record<string, unknown>) => tab.sessionID === "root-a")?.active,
    ).toBe(true);
  } finally {
    console.warn = originalWarn;
  }
});

test("a list read started during the post-focus readback cannot replace the refresh", async () => {
  const staleList = [rootTab("root-a", { active: true }), rootTab("root-c")];
  const freshList = [rootTab("root-a"), rootTab("root-c", { active: true })];
  const plugin = await loadPlugin();
  const bridge = bridgeApi(staleList);
  bridge.tui.addSession({ id: "root-a" });
  await plugin.tui(bridge.tui.api);
  await waitFor(() => metadataRequests().length > 0);
  await waitFor(() => eventStreams.length > 0);
  const token = String(lastTabsToken());

  // The drain's membership read holds first.
  bridge.setHoldList(true);
  emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-c", requestID: hexId(0xe1) }) });
  await waitFor(() => bridge.pendingLists() > 0);

  // The refresh read sees the fresh list; a poll that starts while the refresh
  // is still awaiting sees the stale one.
  bridge.enqueueList(freshList);
  bridge.releaseList(0);
  await waitFor(() => bridge.focusCalls.length === 1);
  bridge.enqueueList(staleList);
  const polled = await waitFor(() => bridge.pendingLists() >= 2, 2_500);
  expect(polled).toBe(true);

  // The fresh snapshot lands first.
  bridge.releaseList(0);
  await waitFor(
    () => lastTabsPayload()?.tabs?.some((tab: Record<string, unknown>) => tab.sessionID === "root-c" && tab.active) === true,
    1_000,
  );

  // The stale poll read resolves afterwards and must be discarded.
  bridge.releaseList(0);
  await sleep(200);
  expect(
    lastTabsPayload().tabs.some((tab: Record<string, unknown>) => tab.sessionID === "root-c" && tab.active),
  ).toBe(true);
});

test("a transient readback failure keeps focus enabled and retries", async () => {
  const warnings: string[] = [];
  const originalWarn = console.warn;
  console.warn = (...args: unknown[]) => {
    warnings.push(args.map(String).join(" "));
  };
  try {
    const plugin = await loadPlugin();
    const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
    bridge.tui.addSession({ id: "root-a" });
    bridge.setFocusActivates(true);
    await plugin.tui(bridge.tui.api);
    await waitFor(() => metadataRequests().length > 0);
    await waitFor(() => eventStreams.length > 0);
    await waitFor(() => requests.filter((request) => isRecord(request) && request.method === "pane.current").length >= 1);
    const token = String(lastTabsToken());

    // Two failures so the post-focus read still misses even if a reconcile
    // read happens to land first.
    bridge.setPaneCurrentFailures(2);
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x111) }) });
    await waitFor(() => bridge.focusCalls.length === 1);

    const transient = await waitFor(() => warnings.some((line) => line.includes("could not verify")));
    expect(transient).toBe(true);
    // A read that failed says nothing about ownership.
    expect(warnings.some((line) => line.includes("lost the tab surface"))).toBe(false);

    // The very next request still works instead of waiting out the token.
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-a", requestID: hexId(0x112) }) });
    await waitFor(() => bridge.focusCalls.length === 2, 2_000);
    expect(bridge.focusCalls).toEqual(["root-b", "root-a"]);
    expect(
      lastTabsPayload().tabs.some((tab: Record<string, unknown>) => tab.sessionID === "root-a" && tab.active),
    ).toBe(true);
  } finally {
    console.warn = originalWarn;
  }
}, 15_000);

test("a definitive readback mismatch still disables focus", async () => {
  const warnings: string[] = [];
  const originalWarn = console.warn;
  console.warn = (...args: unknown[]) => {
    warnings.push(args.map(String).join(" "));
  };
  try {
    const plugin = await loadPlugin();
    const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
    bridge.tui.addSession({ id: "root-a" });
    await plugin.tui(bridge.tui.api);
    await waitFor(() => metadataRequests().length > 0);
    await waitFor(() => eventStreams.length > 0);
    const token = String(lastTabsToken());

    // The pane answers, but the stored token belongs to someone else.
    paneTokens.set(
      "herdr_oc_tabs",
      JSON.stringify({
        v: 1,
        owner: "f".repeat(32),
        tabs: [{ sessionID: "root-a", title: "a", active: true, busy: false, attention: false, unread: false }],
      }),
    );
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x121) }) });
    await waitFor(() => bridge.focusCalls.length === 1);
    const lost = await waitFor(() => warnings.some((line) => line.includes("lost the tab surface")));
    expect(lost).toBe(true);

    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ requestID: hexId(0x122) }) });
    await sleep(200);
    expect(bridge.focusCalls).toEqual(["root-a"]);
  } finally {
    console.warn = originalWarn;
  }
}, 15_000);

test("a focus call that never settles is reported and never overlapped", async () => {
  const warnings: string[] = [];
  const originalWarn = console.warn;
  console.warn = (...args: unknown[]) => {
    warnings.push(args.map(String).join(" "));
  };
  try {
    const plugin = await loadPlugin();
    const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b")]);
    bridge.tui.addSession({ id: "root-a" });
    bridge.setHoldFocus(true);
    await plugin.tui(bridge.tui.api);
    await waitFor(() => metadataRequests().length > 0);
    await waitFor(() => eventStreams.length > 0);
    const token = String(lastTabsToken());

    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x131) }) });
    await waitFor(() => bridge.focusCalls.length === 1);

    // Everything that arrives while the call hangs is queued at most.
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-a", requestID: hexId(0x132) }) });
    const stalled = await waitFor(() => warnings.some((line) => line.includes("did not settle")), 8_000);
    expect(stalled).toBe(true);

    // The watchdog never cancels the host call, launches another, or runs the
    // queued request over it.
    expect(bridge.focusCalls).toEqual(["root-b"]);
    expect(bridge.peakFocus()).toBe(1);
    expect(warnings.some((line) => line.includes("reload required"))).toBe(true);

    // Focus stays closed, so later requests are dropped without a call.
    emitPane({ herdr_oc_tabs: token, herdr_oc_focus: focusPayload({ sessionID: "root-b", requestID: hexId(0x133) }) });
    await sleep(200);
    expect(bridge.focusCalls).toEqual(["root-b"]);

    // A late settlement does not restore focus (fail closed until reload).
    bridge.releaseFocus();
    await sleep(400);
    expect(bridge.focusCalls).toEqual(["root-b"]);
    expect(bridge.peakFocus()).toBe(1);
  } finally {
    console.warn = originalWarn;
  }
}, 30_000);

// The tab list carries a cached mirror of each root's flags. A dropped event
// must not leave a settled tab blocked, so the live APIs win when they answer.
test("publishes live flags instead of the tab list's stale mirror", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { busy: true, attention: true })], {
    live: {
      session: {
        get: (id: string) => ({ id }),
        family: () => [],
        status: () => "idle",
        permission: { async sync() {}, list: () => [] },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const published = await waitFor(() => lastTabsPayload()?.tabs?.[0]?.busy === false);

  expect(published).toBe(true);
  const payload = lastTabsPayload();
  expect(payload.tabs[0].attention).toBe(false);
  expect(payload.tabs[0].busy).toBe(false);
}, 15_000);

test("a running child keeps its root working while the root is idle", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { busy: false })], {
    live: {
      session: {
        get: (id: string) => ({ id, parentID: id === "child" ? "root-a" : undefined }),
        family: () => ["child"],
        status: (id: string) => (id === "child" ? "running" : "idle"),
        permission: { async sync() {}, list: () => [] },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const published = await waitFor(() => lastTabsPayload()?.tabs?.[0]?.busy === true);

  expect(published).toBe(true);
}, 15_000);

test("an unverified descendant does not clear a working tab", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { busy: true })], {
    live: {
      session: {
        get: (id: string) => ({ id, parentID: id === "child" ? "root-a" : undefined }),
        family: () => ["child"],
        status: (id: string) => (id === "child" ? undefined : "idle"),
        permission: { async sync() {}, list: () => [] },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  await sleep(1_500);

  // The root is idle and the child has no status yet: keep the host's busy flag
  // instead of reporting the family as settled.
  expect(lastTabsPayload().tabs[0].busy).toBe(true);
}, 15_000);

test("keeps leaf blockers from the live API on the owning root's tab", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true, attention: false })], {
    live: {
      session: {
        get: (id: string) => ({ id, parentID: id === "child" ? "root-a" : undefined }),
        family: () => ["child"],
        status: () => "idle",
        permission: {
          async sync() {},
          list: () => [{ id: "permission-on-child" }],
        },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const published = await waitFor(() => lastTabsPayload()?.tabs?.[0]?.attention === true);

  expect(published).toBe(true);
}, 15_000);

test("a blocker on an inactive root affects only that tab", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { active: true }), rootTab("root-b", { attention: false })], {
    live: {
      session: {
        get: (id: string) => ({ id }),
        family: () => [],
        status: () => "idle",
        permission: {
          async sync() {},
          list: (id: string) => (id === "root-b" ? [{ id: "inactive-blocker" }] : []),
        },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const published = await waitFor(() => lastTabsPayload()?.tabs?.length === 2);

  expect(published).toBe(true);
  const tabs = lastTabsPayload().tabs;
  expect(tabs.find((tab: Record<string, unknown>) => tab.sessionID === "root-a")?.attention).toBe(false);
  expect(tabs.find((tab: Record<string, unknown>) => tab.sessionID === "root-b")?.attention).toBe(true);
}, 15_000);

test("a failed live refresh keeps the last known blocker", async () => {
  let failReads = false;
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { attention: false })], {
    live: {
      session: {
        get: (id: string) => ({ id }),
        family: () => [],
        status: () => "idle",
        permission: {
          async sync() {},
          list: () => {
            if (failReads) throw new Error("live API unavailable");
            return [{ id: "perm-1" }];
          },
        },
        form: { async sync() {}, list: () => [] },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const blocked = await waitFor(() => lastTabsPayload()?.tabs?.[0]?.attention === true);
  expect(blocked).toBe(true);

  failReads = true;
  await sleep(1_500);
  expect(lastTabsPayload().tabs[0].attention).toBe(true);
}, 15_000);

test("clears a stale cached blocker once the live API answers", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  harness.permissions.set("a", [{ id: "phantom" }]);
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {},
    list: () => [],
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  const settled = await waitFor(() => states().at(-1) === "idle", 3_000);

  expect(settled).toBe(true);
  expect(states().at(-1)).toBe("idle");
}, 15_000);

test("keeps a live child blocker on the pane", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {},
    list: (id: string) => (id === "child" ? [{ id: "permission-on-child" }] : []),
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  const blocked = await waitFor(() => states().at(-1) === "blocked", 3_000);

  expect(blocked).toBe(true);
}, 15_000);

test("a live blocker on another root never blocks the selected pane", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {},
    list: (id: string) => (id === "b" ? [{ id: "other-root" }] : []),
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  await sleep(1_500);

  expect(states().at(-1)).toBe("idle");
}, 15_000);

test("drops an unconfirmed old event delta so a stale cache cannot pin blocked", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  // The cached list is hydrated and never includes the event's request id.
  harness.permissions.set("a", []);

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  await flushReports();

  harness.emit("permission.asked", { id: "ghost", sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");

  // Bounded retention: after the delta TTL the unconfirmed ask stops blocking.
  const settled = await waitFor(() => states().at(-1) === "idle", 20_000);
  expect(settled).toBe(true);
}, 30_000);

test("a read that started before a fresh ask does not discard it", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let syncs = 0;
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {
      syncs += 1;
      if (syncs === 1) await gate;
    },
    // Models a read that began before the ask: it cannot see it.
    list: () => [],
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  await flushReports();

  // The first authoritative read is in flight; the ask arrives meanwhile.
  harness.emit("permission.asked", { id: "fresh", sessionID: "a" });
  await flushReports();
  release();
  await flushReports();

  // The stale read must not clear the ask.
  expect(states().at(-1)).toBe("blocked");
}, 15_000);

test("a partial read does not hide attention behind an empty permission list", async () => {
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { attention: true })], {
    live: {
      session: {
        get: (id: string) => ({ id }),
        family: () => [],
        status: () => "idle",
        permission: { async sync() {}, list: () => [] },
        form: {
          async sync() {},
          list: () => {
            throw new Error("form API unavailable");
          },
        },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);
  const published = await waitFor(() => metadataRequests().length > 0);
  expect(published).toBe(true);

  // One kind confirmed empty cannot answer for the other: keep the host flag.
  await sleep(1_500);
  expect(lastTabsPayload().tabs[0].attention).toBe(true);
}, 15_000);

test("a cached false from a failed refresh cannot clear a newer ask", async () => {
  let failing = false;
  const plugin = await loadPlugin();
  const harness = v2Api();
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {},
    list: () => {
      if (failing) throw new Error("permission API unavailable");
      return [];
    },
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  await sleep(250);
  failing = true;

  // Ask arrives after the last successful (empty) read.
  harness.emit("permission.asked", { id: "fresh-after-failure", sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("blocked");

  // Failed refreshes keep the old false value with its old timestamp, so the
  // delta is not superseded and the ask survives.
  await sleep(1_500);
  expect(states().at(-1)).toBe("blocked");
}, 15_000);

test("a sync that never settles cannot wedge the reader", async () => {
  let syncs = 0;
  const plugin = await loadPlugin();
  const bridge = bridgeApi([rootTab("root-a", { attention: true, busy: true })], {
    live: {
      session: {
        get: (id: string) => ({ id }),
        family: () => [],
        status: () => undefined,
        permission: {
          sync: () => {
            syncs += 1;
            return new Promise(() => {});
          },
          list: () => [],
        },
        form: {
          sync: () => new Promise(() => {}),
          list: () => [],
        },
      },
      listen: () => () => {},
    },
  });
  await plugin.tui(bridge.tui.api);

  // The bounded read times out, the snapshot still publishes on the host's
  // flags, and the member is released for a later retry.
  const published = await waitFor(() => metadataRequests().length > 0, 4_000);
  expect(published).toBe(true);
  expect(lastTabsPayload().tabs[0].attention).toBe(true);
  expect(lastTabsPayload().tabs[0].busy).toBe(true);

  const retried = await waitFor(() => syncs >= 2, 4_000);
  expect(retried).toBe(true);
}, 20_000);

test("a reply is not lost to a read that started before it", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  let releaseRead!: () => void;
  const gate = new Promise<void>((resolve) => {
    releaseRead = resolve;
  });
  let gated = true;
  let failing = false;
  let open: Array<{ id: string }> = [{ id: "ask-1" }];
  // The read snapshots the server state when it starts; `open` may change while
  // the sync is still in flight.
  let captured: Array<{ id: string }> = [];
  (harness.api.data.session as Record<string, unknown>).permission = {
    async sync() {
      captured = open.map((item) => ({ ...item }));
      if (!gated) return;
      gated = false;
      await gate;
    },
    list: () => {
      if (failing) throw new Error("permission API unavailable");
      return captured.map((item) => ({ ...item }));
    },
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  await flushReports();

  // The first read started before this reply and will still report ask-1.
  harness.emit("permission.replied", { requestID: "ask-1", sessionID: "a" });
  // The server applies the reply immediately; only the in-flight read is stale.
  open = [];
  await flushReports();
  releaseRead();
  await flushReports();

  // The answered request must not survive the stale read.
  expect(states().at(-1)).toBe("idle");

  // A later failed refresh must not resurrect it either.
  failing = true;
  await sleep(1_200);
  expect(states().at(-1)).toBe("idle");

  // A genuinely different open request still blocks.
  failing = false;
  open = [{ id: "ask-2" }];
  const blocked = await waitFor(() => states().at(-1) === "blocked", 2_500);
  expect(blocked).toBe(true);

  open = [];
  const cleared = await waitFor(() => states().at(-1) === "idle", 2_500);
  expect(cleared).toBe(true);
}, 20_000);

test("a stale list-only reply stays cleared across repeated reconciles", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  // The host has list() but no sync(): the local list remains stale after the
  // reply. The tombstone must keep suppressing that exact id.
  tui.permissions.set("a", [{ id: "stale-reply" }]);
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  const blocked = await waitFor(() => states().at(-1) === "blocked", 2_000);
  expect(blocked).toBe(true);

  tui.emit("permission.replied", { requestID: "stale-reply", sessionID: "a" });
  const cleared = await waitFor(() => states().at(-1) === "idle", 2_000);
  expect(cleared).toBe(true);
  const afterReply = states().length;

  // Each event and poll reconciles again against the same stale list.
  for (let i = 0; i < 3; i += 1) {
    tui.emit("session.execution.started", { sessionID: "a" });
    await flushReports();
    tui.emit("session.execution.succeeded", { sessionID: "a" });
    await flushReports();
  }
  await sleep(1_200);
  expect(states().slice(afterReply)).not.toContain("blocked");
}, 15_000);

test("a list-only reply survives a quick selection switch", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  tui.permissions.set("a", [{ id: "switch-reply" }]);
  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  const blocked = await waitFor(() => states().at(-1) === "blocked", 2_000);
  expect(blocked).toBe(true);

  tui.emit("permission.replied", { requestID: "switch-reply", sessionID: "a" });
  // Switch away before the reply is reconciled, then return with the list still
  // reporting the answered request.
  tui.select("b");
  tui.emit("session.updated", { sessionID: "b" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");

  tui.select("a");
  tui.emit("session.updated", { sessionID: "a" });
  await flushReports();
  expect(states().at(-1)).toBe("idle");
  await sleep(1_200);
  expect(states().at(-1)).toBe("idle");
}, 15_000);

test("an inactive-root reply is not revived by an old read when later reads fail", async () => {
  const plugin = await loadPlugin();
  const tui = v2Api();
  tui.sessions.set("c", { id: "c" });
  let open: Array<{ id: string }> = [{ id: "ask-b" }];
  let captured: Array<{ id: string }> = [];
  let bStarted = false;
  let releaseB!: () => void;
  const bGate = new Promise<void>((resolve) => {
    releaseB = resolve;
  });
  let failReads = false;
  (tui.api.data.session as Record<string, unknown>).permission = {
    async sync(id: string) {
      if (id !== "b" || bStarted) return;
      captured = open.map((item) => ({ ...item }));
      bStarted = true;
      await bGate;
    },
    list(id: string) {
      if (failReads) throw new Error("permission API unavailable");
      if (id === "b") return captured.map((item) => ({ ...item }));
      if (id === "c") return [{ id: "keep-c" }];
      return [];
    },
  };
  (tui.api.data.session as Record<string, unknown>).form = {
    async sync() {},
    list: () => [],
  };
  (tui.api.ui as Record<string, unknown>).tabs = {
    list: () => [
      rootTab("a", { active: true }),
      rootTab("b", { attention: true }),
      rootTab("c", { attention: true }),
    ],
    focus: () => {},
  };

  const dispose = await plugin.setup(tui.api);
  activeDisposers.push(dispose);
  const started = await waitFor(() => bStarted, 2_000);
  expect(started).toBe(true);

  // Reply for an inactive root while its pre-reply read is still in flight.
  tui.emit("permission.replied", { requestID: "ask-b", sessionID: "b" });
  open = [];
  releaseB();

  const cleared = await waitFor(() => {
    const tabs = lastTabsPayload()?.tabs ?? [];
    const b = tabs.find((tab: Record<string, unknown>) => tab.sessionID === "b");
    const c = tabs.find((tab: Record<string, unknown>) => tab.sessionID === "c");
    return b?.attention === false && c?.attention === true;
  }, 3_000);
  expect(cleared).toBe(true);

  // Failed later reads preserve the cleared reply without clearing the
  // unrelated request on root c.
  failReads = true;
  await sleep(1_800);
  const tabs = lastTabsPayload()?.tabs ?? [];
  expect(tabs.find((tab: Record<string, unknown>) => tab.sessionID === "b")?.attention).toBe(false);
  expect(tabs.find((tab: Record<string, unknown>) => tab.sessionID === "c")?.attention).toBe(true);
}, 20_000);

test("a throwing fallback list keeps the blocker it already holds", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  harness.permissions.set("a", [{ id: "cached-ask" }]);

  const dispose = await plugin.setup(harness.api);
  activeDisposers.push(dispose);
  const blocked = await waitFor(() => states().at(-1) === "blocked", 2_000);
  expect(blocked).toBe(true);

  (harness.api.data.session as Record<string, unknown>).permission = {
    list: () => {
      throw new Error("cache unavailable");
    },
  };
  await sleep(1_200);
  expect(states().at(-1)).toBe("blocked");
}, 15_000);

test("a hanging sync keeps the whole refresh inside its tick budget", async () => {
  const plugin = await loadPlugin();
  const live = {
    session: {
      get: (id: string) => ({ id }),
      family: () => [],
      status: () => undefined,
      permission: { sync: () => new Promise(() => {}), list: () => [] },
      form: { sync: () => new Promise(() => {}), list: () => [] },
    },
    listen: () => () => {},
  };
  const bridge = bridgeApi(
    [rootTab("root-1"), rootTab("root-2"), rootTab("root-3"), rootTab("root-4")],
    { live },
  );
  await plugin.tui(bridge.tui.api);

  // Four members at up to 3s each would need ~12s; the budget ends the pass.
  const started = Date.now();
  const published = await waitFor(() => metadataRequests().length > 0, 8_000);
  expect(published).toBe(true);
  expect(Date.now() - started).toBeLessThan(8_000);
}, 20_000);

test("disposal skips the second SDK call of an in-flight read", async () => {
  const plugin = await loadPlugin();
  const harness = v2Api();
  let formSyncs = 0;
  (harness.api.data.session as Record<string, unknown>).permission = {
    sync: () => new Promise(() => {}),
    list: () => [],
  };
  (harness.api.data.session as Record<string, unknown>).form = {
    async sync() {
      formSyncs += 1;
    },
    list: () => [],
  };

  const dispose = await plugin.setup(harness.api);
  await sleep(150);
  dispose();
  await sleep(1_800);

  expect(formSyncs).toBe(0);
}, 10_000);
