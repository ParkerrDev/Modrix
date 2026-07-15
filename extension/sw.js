// SPDX-License-Identifier: GPL-2.0-only
//
// Modrix Bridge - MV3 background service worker.
//
// It watches the browser's own downloads and, when one looks like a mod
// (an archive, or a Nexus CDN URL, or an explicit "Download with Modrix"
// context-menu click), it cancels the browser download and hands the
// already-authenticated request (URL + cookies + User-Agent + referrer) to the
// local Modrix service over loopback. There is no site API and no API key -
// the browser already did the authentication.
//
// Listeners are registered at the top level (MV3 wakes the worker on the event);
// no state is kept in module globals - the port and token live in storage.local.

const api = globalThis.browser ?? globalThis.chrome;
const ARCHIVE = /\.(zip|7z|rar|fomod|tar\.gz|tar\.bz2)(\?|$)/i;

// --- explicit "Download with Modrix" context-menu (works on any link) ---
api.runtime.onInstalled.addListener(() => {
  api.contextMenus?.create({
    id: "mm-download",
    title: "Download with Modrix",
    contexts: ["link"],
  });
});

api.contextMenus?.onClicked.addListener((info) => {
  if (info.menuItemId === "mm-download" && info.linkUrl) {
    handoff(info.linkUrl, { pageUrl: info.pageUrl });
  }
});

// --- automatic interception of mod downloads -------------------------------
// Chrome: onDeterminingFilename fires before the file commits and exposes the
// final URL + referrer. Firefox lacks it, so we fall back to onCreated.
api.downloads.onDeterminingFilename?.addListener((item, suggest) => {
  if (shouldIntercept(item)) {
    intercept(item);
  } else {
    suggest();
  }
});

api.downloads.onCreated.addListener((item) => {
  if (!api.downloads.onDeterminingFilename && shouldIntercept(item)) {
    intercept(item);
  }
});

function shouldIntercept(item) {
  const url = item.finalUrl || item.url || "";
  if (/^(blob:|data:|about:)/.test(url)) return false;
  const name = item.filename || url;
  return ARCHIVE.test(name) || /\.nexus-cdn\.com/i.test(hostOf(url));
}

async function intercept(item) {
  // Stop the browser writing its own copy, then hand the request off.
  await api.downloads.cancel(item.id).catch(() => {});
  await api.downloads.erase({ id: item.id }).catch(() => {});
  handoff(item.finalUrl || item.url, {
    filename: item.filename,
    totalBytes: item.totalBytes,
    mime: item.mime,
    pageUrl: item.referrer,
  });
}

async function handoff(url, extra = {}) {
  const { port, token } = await api.storage.local.get(["port", "token"]);
  if (!port || !token) {
    notify("Open the Modrix Bridge options and set the port + token from `modrix serve`.");
    return;
  }
  const cookies = await api.cookies.getAll({ url }).catch(() => []);
  const pageUrl = extra.pageUrl || url;
  const job = {
    schemaVersion: 1,
    url,
    filename: basename(extra.filename || pathOf(url)),
    mime: extra.mime,
    referrer: extra.pageUrl,
    userAgent: navigator.userAgent,
    cookie: cookies.map((c) => `${c.name}=${c.value}`).join("; "),
    totalBytes: extra.totalBytes,
    pageUrl,
    gameHint: gameHint(pageUrl),
  };
  try {
    const res = await fetch(`http://127.0.0.1:${port}/download`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-modrix-token": token },
      body: JSON.stringify(job),
    });
    notify(res.ok ? "Sent to Modrix." : `Modrix rejected it: ${await safeText(res)}`);
  } catch (_e) {
    notify("Modrix isn't running - start it with `modrix serve`.");
  }
}

// --- helpers ---------------------------------------------------------------
function gameHint(u) {
  const m = /nexusmods\.com\/([a-z0-9-]+)\//i.exec(u || "");
  return m ? { domain: m[1] } : undefined;
}

function hostOf(u) {
  try {
    return new URL(u).host;
  } catch {
    return "";
  }
}

function pathOf(u) {
  try {
    return new URL(u).pathname;
  } catch {
    return u || "";
  }
}

function basename(p) {
  const last = (p || "").split(/[\\/]/).pop() || "";
  return last.split("?")[0] || "download";
}

async function safeText(res) {
  try {
    return await res.text();
  } catch {
    return "unknown error";
  }
}

function notify(message) {
  // Chrome requires an iconUrl; fall back to the console if notifications fail.
  api.notifications
    ?.create({ type: "basic", iconUrl: api.runtime.getURL("icon.png"), title: "Modrix", message })
    .catch?.(() => console.log("[Modrix]", message));
  console.log("[Modrix]", message);
}
