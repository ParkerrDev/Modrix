// SPDX-License-Identifier: GPL-2.0-only
// Options page: persist the ModManager port + session token and test the link.

const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);

async function load() {
  const { port, token } = await api.storage.local.get(["port", "token"]);
  if (port) $("port").value = port;
  if (token) $("token").value = token;
}

async function save() {
  const port = parseInt($("port").value, 10);
  const token = $("token").value.trim();
  if (!port || !token) {
    setStatus("Enter both a port and a token.");
    return;
  }
  await api.storage.local.set({ port, token });
  setStatus("Saved.");
}

async function test() {
  const port = parseInt($("port").value, 10);
  const token = $("token").value.trim();
  try {
    const res = await fetch(`http://127.0.0.1:${port}/downloads`, {
      headers: { "x-modman-token": token },
    });
    setStatus(res.ok ? "Connected to ModManager." : `ModManager replied ${res.status}.`);
  } catch (_e) {
    setStatus("Could not reach ModManager - is `modman serve` running?");
  }
}

function setStatus(text) {
  $("status").textContent = text;
}

$("save").addEventListener("click", save);
$("test").addEventListener("click", test);
load();
