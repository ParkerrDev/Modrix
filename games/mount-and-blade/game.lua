-- SPDX-License-Identifier: GPL-2.0-only
-- Mount & Blade install layout, ported from Vortex's game-mount-and-blade.
--
-- Two mod shapes deploy into Modules/:
--   * a full module ships a module.ini - keep the folder that holds it so it
--     lands as Modules/<folder>/...
--   * a loose "override" mod ships recognised asset files - route them into the
--     native module's subfolders by extension (Vortex's MOD_EXT_DESTINATION).
-- Anything else returns false and defers to default normalization.

local NATIVE = "Native"

-- extension (lowercase, no dot) -> native-module subfolder ("" = module root)
local EXT_DEST = {
  dds = "Textures",
  brf = "Resource",
  sco = "SceneObj",
  txt = "",
}

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

function install()
  local files = collect_files()

  -- Module mod: locate module.ini and keep its containing folder.
  local module_ini = nil
  for _, f in ipairs(files) do
    if string.lower(basename(f)) == "module.ini" then
      module_ini = f
      break
    end
  end

  if module_ini ~= nil then
    local root = dirname(module_ini)
    if root == "" then
      -- module.ini at the archive root: nothing to preserve, deploy verbatim.
      for _, f in ipairs(files) do
        modrix.fs.stage(f, f)
      end
    else
      local prefix = root .. "/"
      local folder = basename(root)
      for _, f in ipairs(files) do
        if string.sub(f, 1, #prefix) == prefix then
          modrix.fs.stage(f, folder .. "/" .. string.sub(f, #prefix + 1))
        end
      end
    end
    return true
  end

  -- Override mod: route recognised asset files into the native module.
  local staged = false
  for _, f in ipairs(files) do
    local sub = EXT_DEST[lower_ext(f)]
    if sub ~= nil then
      local dest
      if sub == "" then
        dest = NATIVE .. "/" .. basename(f)
      else
        dest = NATIVE .. "/" .. sub .. "/" .. basename(f)
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
