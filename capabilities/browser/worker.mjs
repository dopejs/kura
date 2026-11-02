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
// not resolvable from this file), KURA_BROWSER_HEADLESS (default "1"),
// KURA_BROWSER_DOWNLOAD_DIR (set: downloads are saved there and reported by
// path; unset: returned inline) and KURA_BROWSER_MAX_DOWNLOAD_BYTES (default
// 50 MiB).

import { createInterface } from "node:readline";
import { createRequire } from "node:module";
import { mkdir, stat, writeFile, copyFile } from "node:fs/promises";
import path from "node:path";

const MAX_DOWNLOAD_BYTES = Number(process.env.KURA_BROWSER_MAX_DOWNLOAD_BYTES) || 50 * 1024 * 1024;

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

// In a tenant container the egress proxy arrives as HTTPS_PROXY, with the
// tenant's credential as userinfo. Chromium ignores userinfo in a proxy URL,
// so it is handed to Playwright, which answers the proxy's 407 challenge.
function proxyFromEnv() {
  const raw = process.env.HTTPS_PROXY || process.env.https_proxy;
  if (!raw) return {};
  let url;
  try {
    url = new URL(raw);
  } catch {
    return {};
  }
  const proxy = { server: `${url.protocol}//${url.host}` };
  if (url.username) {
    proxy.username = decodeURIComponent(url.username);
    proxy.password = decodeURIComponent(url.password);
  }
  return { proxy };
}

async function ensureBrowser() {
  if (state.browser) return state.browser;
  const { chromium } = loadPlaywright();
  const headless = (process.env.KURA_BROWSER_HEADLESS ?? "1") !== "0";
  state.browser = await chromium.launch({ headless, ...proxyFromEnv() });
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

// A name that stays inside the download directory: no separators, no
// leading dots, no control characters, bounded length.
function safeFileName(raw) {
  let name = path.basename(String(raw || "").replace(/\\/g, "/"));
  name = name.replace(/[\x00-\x1f\x7f/:*?"<>|]/g, "_").replace(/^\.+/, "").trim();
  if (name.length > 120) {
    const ext = path.extname(name).slice(0, 16);
    name = name.slice(0, 120 - ext.length) + ext;
  }
  return name || "download";
}

const EXTENSIONS = {
  "text/html": ".html", "text/plain": ".txt", "text/csv": ".csv", "application/pdf": ".pdf",
  "application/json": ".json", "application/xml": ".xml", "text/xml": ".xml", "application/zip": ".zip",
  "image/png": ".png", "image/jpeg": ".jpg", "image/gif": ".gif", "image/webp": ".webp", "image/svg+xml": ".svg",
};

// A name without an extension gets one from the content type, so tools that
// go by extension know what the file is.
function withExtension(name, mimeType) {
  if (path.extname(name)) return name;
  return name + (EXTENSIONS[mimeType] || "");
}

function nameFromDisposition(header) {
  if (!header) return "";
  const star = /filename\*\s*=\s*(?:UTF-8'')?([^;]+)/i.exec(header);
  if (star) {
    try {
      return decodeURIComponent(star[1].trim().replace(/^"|"$/g, ""));
    } catch {}
  }
  const plain = /filename\s*=\s*("?)([^";]+)\1/i.exec(header);
  return plain ? plain[2].trim() : "";
}

function nameFromUrl(url) {
  try {
    return decodeURIComponent(path.basename(new URL(url).pathname));
  } catch {
    return "";
  }
}

// Writes without replacing: report.pdf, report-1.pdf, report-2.pdf, ...
async function claimPath(dir, name, write) {
  await mkdir(dir, { recursive: true });
  const ext = path.extname(name);
  const stem = name.slice(0, name.length - ext.length);
  for (let n = 0; n < 1000; n++) {
    const candidate = path.join(dir, n === 0 ? name : `${stem}-${n}${ext}`);
    try {
      await write(candidate);
      return candidate;
    } catch (err) {
      if (err?.code !== "EEXIST") throw err;
    }
  }
  throw new Error("too many files with this name in the download directory");
}

function tooLarge(bytes) {
  return new Error(`the file is ${bytes} bytes; the limit is ${MAX_DOWNLOAD_BYTES}`);
}

// A file by URL (fetched in the page's browser context: same proxy and
// cookies) or by clicking a link that starts a download.
async function downloadCapture(page, action) {
  const url = inputString(action, "url");
  const selector = selectorFor(action);
  const dir = process.env.KURA_BROWSER_DOWNLOAD_DIR;
  let name;
  let mimeType = "application/octet-stream";
  let body = null; // Buffer, or null when the file is on disk at `source`
  let source = "";
  let size = 0;
  if (url) {
    const response = await page.context().request.get(url, { timeout: 30000, maxRedirects: 5 });
    if (!response.ok()) throw new Error(`the server answered HTTP ${response.status()}`);
    const headers = response.headers();
    const declared = Number(headers["content-length"] || 0);
    if (declared > MAX_DOWNLOAD_BYTES) throw tooLarge(declared);
    body = await response.body();
    size = body.length;
    mimeType = (headers["content-type"] || mimeType).split(";")[0].trim() || mimeType;
    name = nameFromDisposition(headers["content-disposition"]) || nameFromUrl(response.url()) || nameFromUrl(url);
  } else if (selector) {
    const [download] = await Promise.all([
      page.waitForEvent("download", { timeout: 30000 }),
      page.click(selector, { timeout: 10000 }),
    ]);
    const failure = await download.failure();
    if (failure) throw new Error(`the download failed: ${failure}`);
    source = await download.path();
    size = (await stat(source)).size;
    name = download.suggestedFilename();
  } else {
    throw new Error("download needs a url or a selector");
  }
  if (size > MAX_DOWNLOAD_BYTES) throw tooLarge(size);
  name = withExtension(safeFileName(name), mimeType);
  if (!dir) {
    if (body === null) {
      const { readFile } = await import("node:fs/promises");
      body = await readFile(source);
    }
    return { kind: "download", mimeType, fileName: name, contentBase64: body.toString("base64") };
  }
  const saved = await claimPath(dir, name, (candidate) =>
    body === null ? copyFile(source, candidate, 1 /* COPYFILE_EXCL */) : writeFile(candidate, body, { flag: "wx" }),
  );
  // Reported by path, without content: the file is already where the caller
  // wants it.
  return { kind: "download", mimeType, fileName: saved, contentBase64: "", byteSize: size };
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
        captures.push(await downloadCapture(page, action));
        break;
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
      : action.actionKind === "download"
        ? "download_failure"
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
