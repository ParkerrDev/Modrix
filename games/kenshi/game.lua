-- SPDX-License-Identifier: GPL-2.0-only
-- Kenshi install layout, ported from Vortex's game-kenshi installer.
--
-- Kenshi loads a mod from mods/<name>/<name>.mod, where <name> must match the
-- .mod file's base name. We derive <name> from the .mod file inside the archive
-- and re-root every file that sits at or below the .mod file's directory under
-- that folder. Returning true takes over staging; if there is no .mod file we
-- return false and let default normalization handle it.

local MAX_DIRS = 4000

local function join(a, b)
  if a == "" then return b end
  return a .. "/" .. b
end

local function basename(p)
  return string.match(p, "[^/]+$") or p
end

local function dirname(p)
  return string.match(p, "^(.*)/[^/]+$") or ""
end

-- A relative path is a directory if read_dir yields at least one entry.
local function is_dir(rel)
  local entries = modrix.fs.read_dir(rel)
  return entries ~= nil and #entries > 0
end

-- Flatten every file under the archive root (bounded directory worklist).
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

function install()
  local files = collect_files()

  local mod_file = nil
  for _, f in ipairs(files) do
    if string.match(string.lower(f), "%.mod$") ~= nil then
      mod_file = f
      break
    end
  end
  if mod_file == nil then
    return false
  end

  local mod_name = string.gsub(basename(mod_file), "%.[^.]+$", "")
  local root = dirname(mod_file)
  local prefix = (root == "") and "" or (root .. "/")

  for _, f in ipairs(files) do
    if root == "" or string.sub(f, 1, #prefix) == prefix then
      local rel = (root == "") and f or string.sub(f, #prefix + 1)
      modrix.fs.stage(f, mod_name .. "/" .. rel)
    end
  end
  return true
end
