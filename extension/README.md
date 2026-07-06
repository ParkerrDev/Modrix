<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Browser integration

ModManager handles Nexus "Download with Manager" (`nxm://`) links two ways. You
only need one.

## 1. OS protocol handler (recommended - no browser add-on)

Register ModManager as the system handler for the `nxm` scheme:

```sh
modman-protocol --register      # Linux: writes a .desktop + xdg-mime default
```

On Windows/macOS `--register` prints the manual steps (registry / Info.plist).
After that, clicking **Mod Manager Download** on Nexus launches
`modman-protocol`, which forwards the link to the running ModManager service
(start it with `modman serve`, or the GUI once it ships). The download and
install happen with no window open.

## 2. Userscript (the enhanced path)

`modmanager.user.js` is a Tampermonkey/Violentmonkey userscript that forwards
`nxm://` links to the local service directly, without the OS handler - useful if
you can't register the protocol, or as groundwork for non-Nexus sites.

1. Run `modman serve`; it prints a **port** and a **session token**.
2. Install the userscript and edit `MODMANAGER_PORT` / `MODMANAGER_TOKEN` at the
   top to match.
3. Reload the Nexus page. Clicking a Mod Manager Download now POSTs the link to
   `http://127.0.0.1:<port>/nxm` with your token (via `GM_xmlhttpRequest`, which
   bypasses page CORS / mixed-content rules).

The service is **loopback-only and token-authed**: only a client that knows your
per-session token can reach the engine.

> Status: the userscript is a v1 draft and needs verification against the live
> Nexus page structure. A packaged WebExtension follows in Phase 6.
