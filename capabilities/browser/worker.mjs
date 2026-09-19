#!/usr/bin/env node
// Kura browser capability worker (Stage 9.4).
//
// A supervised child process that drives a real Chromium through Playwright
// and speaks the daemon's line-JSON driver protocol on stdio (see
// PROTOCOL.md). The daemon's `SubprocessDriver` owns supervision: it times
// requests out, respawns this process when it dies, and reports both to the
// capability supervisor. This file therefore does no retry logic of its own;
// it answers one request per line, in order, and exits when stdin closes.
//
// The worker holds no credentials and reads no config beyond its
// environment: KURA_PLAYWRIGHT_PATH (optional module path when playwright is
// not resolvable from this file), KURA_BROWSER_HEADLESS (default "1").

import { createInterface } from "node:readline";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

function loadPlaywright() {
  const candidates = [process.env.KURA_PLAYWRIGHT_PATH, "playwright", "playwright-core"].filter(Boolean);
  for (const candidate of candidates) {
    try {
      return require(candidate);
    } catch (err) {
      if (candidate === candidates[candidates.length - 1]) {
        throw new Error(`playwright is not installed (tried ${candidates.join(", ")}): ${err.message}`);
      }
    }
  }
  throw new Error("playwright is not installed");
}

const state = {
  browser: null,
  context: null,
  // computerUseSessionId -> { page, history: [] }
  sessions: new Map(),
};

async function ensureBrowser() {
  if (state.browser) return state.browser;
  const { chromium } = loadPlaywright();
  const headless = (process.env.KURA_BROWSER_HEADLESS ?? "1") !== "0";
  state.browser = await chromium.launch({ headless });
  state.context = await state.browser.newContext();
  return state.browser;
}

function pageSummary(page) {
  return { url: page.url(), title: null };
}

async function summarize(page) {
  let title = "";
  try {
    title = await page.title();
  } catch {
    title = "";
  }
  return { url: page.url(), title };
}

function nowIso() {
  return new Date().toISOString();
}

function trustedScope(session, actionId, page) {
  const revision = (session.trustedPageScope?.scopeRevision ?? 0) + 1;
  let origin = "";
  try {
    origin = new URL(page.url).origin;
  } catch {
    origin = "";
  }
  return {
    scopeId: `tps_${session.computerUseSessionId}_${revision}`,
    computerUseSessionId: session.computerUseSessionId,
    origin,
    pageUrl: page.url,
    title: page.title,
    scopeRevision: revision,
    derivedFromActionId: actionId,
    createdAt: nowIso(),
  };
}

function fail(action, failureClass, reason) {
  const t = nowIso();
  return {
    ...action,
    status: "failed",
    failureClass,
    failureReason: reason,
    updatedAt: t,
    completedAt: t,
  };
}

function complete(action) {
  const t = nowIso();
  return { ...action, status: "completed", updatedAt: t, completedAt: t };
}

function inputString(action, key) {
  const value = action.input?.[key];
  return typeof value === "string" ? value.trim() : "";
}

function selectorFor(action) {
  return inputString(action, "selector") || action.targetMatchContext?.expectedSelector || "";
}

async function startSession(session, input) {
  await ensureBrowser();
  const page = await state.context.newPage();
  state.sessions.set(session.computerUseSessionId, { page });
  const next = { ...session, status: "active", driverKind: input?.driverKind || "browser", updatedAt: nowIso() };
  if (input?.initialUrl) {
    await page.goto(input.initialUrl, { waitUntil: "domcontentloaded" });
    const summary = await summarize(page);
    next.currentPage = summary;
    next.trustedPageScope = trustedScope(next, "", summary);
  }
  return next;
}

async function closeSession(session) {
  const entry = state.sessions.get(session.computerUseSessionId);
  if (entry) {
    try {
      await entry.page.close();
    } catch {}
    state.sessions.delete(session.computerUseSessionId);
  }
  const t = nowIso();
  return { ...session, status: "closed", closedAt: t, updatedAt: t };
}

async function snapshotCapture(page, session, action) {
  const text = await page.evaluate(() => (document.body ? document.body.innerText.slice(0, 20000) : ""));
  const summary = await summarize(page);
  const json = JSON.stringify({
    sessionId: session.computerUseSessionId,
    actionId: action.computerUseActionId,
    actionKind: action.actionKind,
    url: summary.url,
    title: summary.title,
    text,
  });
  return {
    kind: "page_snapshot",
    mimeType: "application/json",
    fileName: "page-snapshot.json",
    contentBase64: Buffer.from(json + "\n", "utf8").toString("base64"),
  };
}

async function screenshotCapture(page) {
  const png = await page.screenshot({ type: "png", fullPage: false });
  return {
    kind: "screenshot",
    mimeType: "image/png",
    fileName: "screenshot.png",
    contentBase64: Buffer.from(png).toString("base64"),
  };
}

async function executeAction(session, action) {
  const entry = state.sessions.get(session.computerUseSessionId);
  if (!entry) {
    return { session, action: fail(action, "unavailable_consumer", "no live page for session"), captures: [] };
  }
  const { page } = entry;
  const captures = [];
  let next = { ...session, updatedAt: nowIso() };
  action = { ...action, status: "running", updatedAt: nowIso(), pageBefore: await summarize(page) };

  const mismatched = action.targetMatchContext?.matchResult === "mismatched";
  try {
    switch (action.actionKind) {
      case "navigate": {
        const url = inputString(action, "url");
        if (!url) return { session: next, action: fail(action, "navigation_failure", "navigate action requires url"), captures };
        await page.goto(url, { waitUntil: "domcontentloaded" });
        break;
      }
      case "back":
        if (!(await page.goBack({ waitUntil: "domcontentloaded" })))
          return { session: next, action: fail(action, "navigation_failure", "back action requires prior page history"), captures };
        break;
      case "forward":
        if (!(await page.goForward({ waitUntil: "domcontentloaded" })))
          return { session: next, action: fail(action, "navigation_failure", "forward action requires forward page history"), captures };
        break;
      case "wait": {
        const ms = Number(action.input?.waitMs ?? 0);
        await page.waitForTimeout(Math.min(Math.max(ms, 0), 30000));
        break;
      }
      case "screenshot":
        captures.push(await screenshotCapture(page));
        break;
      case "snapshot":
        captures.push(await snapshotCapture(page, session, action));
        break;
      case "click":
      case "input":
      case "select": {
        if (mismatched) {
          captures.push(await snapshotCapture(page, session, action));
          return { session: next, action: fail(action, "target_mismatch", "approved target no longer matches current page"), captures };
        }
        const selector = selectorFor(action);
        if (!selector) return { session: next, action: fail(action, "unsupported_action", `${action.actionKind} requires a selector`), captures };
        if (action.actionKind === "click") await page.click(selector, { timeout: 10000 });
        else if (action.actionKind === "input") await page.fill(selector, inputString(action, "value"), { timeout: 10000 });
        else await page.selectOption(selector, inputString(action, "selectedValue"), { timeout: 10000 });
        captures.push(await snapshotCapture(page, session, action));
        break;
      }
      case "download":
        return { session: next, action: fail(action, "unsupported_action", "download is not supported by the browser worker yet"), captures };
      case "close_session": {
        next = await closeSession(next);
        return { session: next, action: complete({ ...action, pageAfter: action.pageBefore }), captures };
      }
      default:
        return { session: next, action: fail(action, "unsupported_action", `unknown action kind ${action.actionKind}`), captures };
    }
  } catch (err) {
    const cls = action.actionKind === "navigate" || action.actionKind === "back" || action.actionKind === "forward"
      ? "navigation_failure"
      : "target_mismatch";
    return { session: next, action: fail(action, cls, String(err?.message ?? err)), captures };
  }

  const after = await summarize(page);
  action = complete({ ...action, pageAfter: after });
  next.currentPage = after;
  next.lastActionId = action.computerUseActionId;
  next.status = "active";
  if (["navigate", "back", "forward"].includes(action.actionKind)) {
    next.trustedPageScope = trustedScope(next, action.computerUseActionId, after);
  }
  return { session: next, action, captures };
}

async function handle(request) {
  const { id, op, session } = request;
  try {
    switch (op) {
      case "ping":
        return { id, ok: true };
      case "start_session":
        return { id, ok: true, session: await startSession(session, request.input ?? {}) };
      case "execute_action": {
        const result = await executeAction(session, request.action);
        return { id, ok: true, ...result };
      }
      case "close_session":
        return { id, ok: true, session: await closeSession(session) };
      default:
        return { id, ok: false, error: `unknown op: ${op}` };
    }
  } catch (err) {
    return { id, ok: false, error: String(err?.message ?? err) };
  }
}

async function main() {
  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
  // Requests are answered strictly in order: the daemon holds one exchange
  // at a time, so a queue keeps the protocol simple and deterministic.
  let chain = Promise.resolve();
  rl.on("line", (line) => {
    if (!line.trim()) return;
    chain = chain.then(async () => {
      let request;
      try {
        request = JSON.parse(line);
      } catch (err) {
        process.stdout.write(JSON.stringify({ id: 0, ok: false, error: `bad request: ${err.message}` }) + "\n");
        return;
      }
      const response = await handle(request);
      process.stdout.write(JSON.stringify(response) + "\n");
    });
  });
  rl.on("close", async () => {
    await chain;
    try {
      if (state.browser) await state.browser.close();
    } catch {}
    process.exit(0);
  });
}

main();
