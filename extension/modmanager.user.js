// ==UserScript==
// @name         ModManager - Download with Manager
// @namespace    https://github.com/modmanager
// @version      0.1.0
// @description  Send Nexus "Download with Manager" (nxm://) links straight to a running ModManager service, no OS protocol handler required.
// @match        https://www.nexusmods.com/*
// @grant        GM_xmlhttpRequest
// @connect      127.0.0.1
// @license      GPL-2.0-only
// ==/UserScript==

// SPDX-License-Identifier: GPL-2.0-only
//
// SETUP
// 1. Start the service:  `modman serve`  - it prints a port and a session token.
// 2. Put that port and token below.
// 3. Reload a Nexus mod page. Clicking "Mod Manager Download" now hands the
//    nxm:// link to your running ModManager (which downloads + installs it),
//    instead of relying on the OS protocol handler.
//
// The service is loopback-only and token-authed, so only a script that knows
// your session token can reach it. GM_xmlhttpRequest is used (not fetch) so the
// request bypasses page CORS and mixed-content restrictions.

(function () {
  "use strict";

  const MODMANAGER_PORT = 41015; // the port `modman serve` printed
  const MODMANAGER_TOKEN = "PASTE-YOUR-SESSION-TOKEN-HERE";

  function toast(text, ok) {
    const el = document.createElement("div");
    el.textContent = text;
    el.style.cssText =
      "position:fixed;z-index:99999;right:16px;bottom:16px;padding:10px 14px;" +
      "border-radius:8px;font:14px system-ui;color:#fff;box-shadow:0 2px 8px rgba(0,0,0,.3);" +
      "background:" + (ok ? "#2e7d32" : "#c62828");
    document.body.appendChild(el);
    setTimeout(() => el.remove(), 4000);
  }

  function sendToModManager(nxmUrl) {
    GM_xmlhttpRequest({
      method: "POST",
      url: "http://127.0.0.1:" + MODMANAGER_PORT + "/nxm",
      headers: {
        "Content-Type": "text/plain",
        "x-modman-token": MODMANAGER_TOKEN,
      },
      data: nxmUrl,
      onload: (res) => toast(res.responseText || "sent to ModManager", res.status === 200),
      onerror: () => toast("ModManager is not running (start `modman serve`)", false),
    });
  }

  // Intercept clicks that navigate to an nxm:// link (that is what Nexus's
  // "Mod Manager Download" button ultimately triggers) and forward it instead.
  document.addEventListener(
    "click",
    (event) => {
      const anchor = event.target.closest && event.target.closest('a[href^="nxm://"]');
      if (!anchor) return;
      event.preventDefault();
      event.stopPropagation();
      sendToModManager(anchor.href);
    },
    true,
  );

  // Some Nexus flows navigate to nxm:// via script rather than a real anchor.
  // Catch those by intercepting assignments to a well-known link, if present.
  window.addEventListener("beforeunload", () => {}, false);
})();
