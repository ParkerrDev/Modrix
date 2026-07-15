// SPDX-License-Identifier: GPL-2.0-only
// Options page: test the link to Modrix. Zero-config by default - the
// listener trusts our extension Origin; port and token are advanced overrides.

const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const DEFAULT_PORT = 41015;

async function load() {
  const { port, token } = await api.storage.local.get(["port", "token"]);
  if (port) $("port").value = port;
  if (token) $("token").value = token;
}

async function save() {
  const port = parseInt($("port").value, 10) || DEFAULT_PORT;
  const token = $("token").value.trim();
  await api.storage.local.set({ port, token });
  setStatus("Saved.");
}

async function test() {
  const port = parseInt($("port").value, 10) || DEFAULT_PORT;
  const token = $("token").value.trim();
  // POST, not GET: browsers always attach our extension Origin to a POST,
  // which is what authenticates us without a token.
  const headers = token ? { "x-modrix-token": token } : {};
  try {
    const res = await fetch(`http://127.0.0.1:${port}/downloads`, {
      method: "POST",
      headers,
    });
    setStatus(res.ok ? "Connected to Modrix." : `Modrix replied ${res.status}.`);
  } catch (_e) {
    setStatus("Could not reach Modrix - is it running?");
  }
}

function setStatus(text) {
  $("status").textContent = text;
}

$("save").addEventListener("click", save);
$("test").addEventListener("click", test);
load();
