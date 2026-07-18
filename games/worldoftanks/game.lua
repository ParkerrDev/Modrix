-- SPDX-License-Identifier: GPL-2.0-only
-- World of Tanks resolves its mod directory as res_mods/<version>, where the
-- version string lives in version.xml at the install root, e.g.
--   <version.xml><version>v.1.20.0.1 #386</version></version.xml>
-- This mirrors Vortex's queryModPath, which reads version.xml, strips the
-- "v." prefix and trailing build tag, and joins res_mods/<version>.

function mod_root(install)
  if modrix.fs.exists("version.xml") then
    local text = modrix.fs.read_text("version.xml")
    if text ~= nil and text ~= "" then
      -- Capture the numeric version that follows "v." in the <version> element.
      local ver = text:match("v%.([%d%.]+)")
      if ver ~= nil then
        -- Drop a trailing dot if the greedy match captured one.
        ver = ver:gsub("%.$", "")
        modrix.log.info("World of Tanks version resolved to " .. ver)
        return "res_mods/" .. ver
      end
    end
  end
  modrix.log.warn("World of Tanks: could not resolve version from version.xml; using res_mods")
  return "res_mods"
end
