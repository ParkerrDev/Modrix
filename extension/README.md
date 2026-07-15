<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Modrix Bridge (browser extension)

Modrix has **no Nexus API and needs no API key**. It is a download manager
fed by this browser extension: you stay logged into Nexus (or any site) in your
own browser, click the normal **Download**, and the extension hands that
already-authenticated download - the real CDN URL plus your cookies, referrer,
and User-Agent - to the local Modrix service, which downloads it (segmented,
resumable) and installs it into the right game.

This is the same idea as Motrix/JDownloader, reimplemented in Rust with no aria2
binary.

## Setup

1. Have Modrix running - the GUI, or headless:
   ```sh
   modrix serve
   ```
2. Load the extension:
   - **Chrome/Edge**: `chrome://extensions` → enable Developer mode → *Load
     unpacked* → select this `extension/` folder.
   - **Firefox**: `about:debugging` → *This Firefox* → *Load Temporary Add-on* →
     select `extension/manifest.json`.

That's it - no token, no pairing. The listener recognizes requests coming from
a browser extension by their `Origin` and lets them through; the options page
(toolbar icon) has a *Test connection* button and advanced overrides (port,
optional token) if you ever need them.

## Using it

- **Automatic**: clicking Nexus's *Manual Download* (or any archive download -
  `.zip`/`.7z`/`.rar`/`.fomod`) hands it to Modrix instead of your browser's
  download folder. The mod is downloaded and staged into the game whose Nexus
  domain the page URL identifies.
- **Explicit**: right-click any link → *Download with Modrix*.

The connection is **loopback-only**. A request is accepted when its `Origin`
is a browser-extension origin (`chrome-extension://…`, `moz-extension://…`) -
which web pages cannot forge - or when it carries the per-session
`x-modrix-token` (how the CLI and `modrix-protocol` authenticate). A drive-by
website therefore still cannot reach the engine. Cookies are only read for the
download's own host and are forwarded solely so an authenticated download
replays correctly.

## Notes

- `nxm://` links are **no longer** a download mechanism (resolving them requires
  the site session, which lives in the browser - exactly what this extension
  captures). `modrix-protocol` and the `nxm://` parser are retained only to read
  game/mod identity.
- This is a v1 draft and needs verification against live sites' download flows.
  A packaged (signed) build follows in a later phase.
