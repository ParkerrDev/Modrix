-- SPDX-License-Identifier: GPL-2.0-only
-- Pillars of Eternity II: Deadfire - resolve the mod directory by store variant.
--
-- Vortex's queryModPath returned "PillarsOfEternity2_Data/override" for the
-- Xbox Game Pass build (its path contains "ModifiableWindowsApps") and
-- "PillarsOfEternityII_Data/override" otherwise. We probe the data folder that
-- actually exists on disk, which is equivalent and does not depend on how the
-- install path happens to be spelled. The game.toml mod_root is the Steam/GOG
-- default and is used whenever the Xbox folder is absent.

function mod_root(install)
  if modrix.fs.exists("PillarsOfEternity2_Data") then
    return "PillarsOfEternity2_Data/override"
  end
  return "PillarsOfEternityII_Data/override"
end
