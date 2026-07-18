-- SPDX-License-Identifier: GPL-2.0-only
-- Neverwinter Nights install layout, ported from Vortex's
-- game-neverwinter-nights installer.
--
-- Loose mod files are routed into the game's content folders by extension;
-- files already inside an override/ folder are kept there verbatim. If the
-- archive already ships a recognised content-folder layout, we return false and
-- defer to default normalization (content_dirs in game.toml keeps those dirs).

-- extension (lowercase, no dot) -> destination content folder
local DEST = {
  mod = "modules", tga = "portraits", erf = "erf", hak = "hak", exe = "hak",
  hif = "hak", tlk = "tlk", bmu = "music", wav = "ambient", cdx = "database",
  dbf = "database", fpt = "database", nbm = "movies", bik = "movies",
  ["2da"] = "override", uti = "override", txi = "override", mdl = "override",
  ncs = "override", dlg = "override", utp = "override",
}

-- directory names that mean the archive is already correctly structured
local MOD_DIRS = {
  ambient = true, database = true, development = true, dmvault = true,
  hak = true, localvault = true, logs = true, modules = true, movies = true,
  music = true, nwsync = true, override = true, portraits = true,
  servervault = true, tempclient = true, tlk = true,
}

local MAX_DIRS = 4000

local function join(a, b)
  if a == "" then return b end
  return a .. "/" .. b
end

local function basename(p)
  return string.match(p, "[^/]+$") or p
end

local function lower_ext(p)
  local e = string.match(p, "%.([^./]+)$")
  return e ~= nil and string.lower(e) or ""
end

local function is_dir(rel)
  local entries = modrix.fs.read_dir(rel)
  return entries ~= nil and #entries > 0
end

local function collect_files()
  local files = {}
  local stack = { "" }
  local guard = 0
  while #stack > 0 and guard < MAX_DIRS do
    guard = guard + 1
    local dir = table.remove(stack)
    for _, name in ipairs(modrix.fs.read_dir(dir)) do
      local rel = join(dir, name)
      if is_dir(rel) then
        stack[#stack + 1] = rel
      else
        files[#files + 1] = rel
      end
    end
  end
  return files
end

-- True if any path component is a known content directory (not a filename).
local function has_correct_layout(files)
  for _, f in ipairs(files) do
    for seg in string.gmatch(f, "[^/]+") do
      if MOD_DIRS[string.lower(seg)] and string.find(seg, "%.") == nil then
        return true
      end
    end
  end
  return false
end

function install()
  local files = collect_files()
  if has_correct_layout(files) then
    return false
  end

  local staged = false
  for _, f in ipairs(files) do
    local dst = DEST[lower_ext(f)]
    if dst ~= nil then
      local dest
      if string.find(string.lower(f), "override", 1, true) ~= nil then
        dest = f
      else
        dest = dst .. "/" .. basename(f)
      end
      modrix.fs.stage(f, dest)
      staged = true
    end
  end
  if staged then
    return true
  end
  return false
end
