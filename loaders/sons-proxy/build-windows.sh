#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
#
# Builds the Sons of the Forest proxy DLL and stages an installable `_Modrix`
# tree under `dist/`. Nothing is ever written outside this directory - installing
# into a game folder is a deliberate, manual step (see README.md), because the
# proxy slot is single-occupancy and overwriting another loader's `version.dll`
# breaks both.
#
#   ./build-windows.sh                            # proxy only
#   ./build-windows.sh --managed                  # + Modrix.Loader.dll
#   ./build-windows.sh --coreclr <mini-coreclr/win-x64>   # + the runtime
#
# Cross-compiles from Linux with the mingw-w64 linker by default. On Windows, or
# with the MSVC toolchain installed, pass MODRIX_PROXY_TARGET=x86_64-pc-windows-msvc.

set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
MANAGED_DIR="$(cd -- "$HERE/../sons-managed/Modrix.Loader" && pwd)"
TARGET="${MODRIX_PROXY_TARGET:-x86_64-pc-windows-gnu}"
PROFILE="${MODRIX_PROXY_PROFILE:-release}"
DIST="$HERE/dist"

want_managed=0
coreclr_src=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --managed) want_managed=1; shift ;;
        --coreclr) coreclr_src="${2:-}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

echo "==> building $TARGET ($PROFILE)"
cargo build --profile "$PROFILE" --target "$TARGET" --manifest-path "$HERE/Cargo.toml"

built="$HERE/target/$TARGET/$PROFILE/modrix_sons_proxy.dll"
[[ -f "$built" ]] || { echo "expected $built" >&2; exit 1; }

# The Windows loader keys on the FILE name, not on the PE export-directory name,
# which is why renaming the artefact is all that is needed to occupy the slot.
# Doorstop ships the identical binary as winhttp.dll and RedLoader renames it.
rm -rf "$DIST"
mkdir -p "$DIST/_Modrix/net6" "$DIST/_Modrix/dotnet"
cp "$built" "$DIST/version.dll"
echo "==> staged $DIST/version.dll"

echo "==> verifying the seventeen version.dll exports"
python3 - "$DIST/version.dll" <<'PY'
import sys
try:
    import pefile
except ImportError:
    print("    SKIPPED (python module `pefile` is not installed)")
    sys.exit(0)

expected = {
    "GetFileVersionInfoA", "GetFileVersionInfoByHandle", "GetFileVersionInfoExA",
    "GetFileVersionInfoExW", "GetFileVersionInfoSizeA", "GetFileVersionInfoSizeExA",
    "GetFileVersionInfoSizeExW", "GetFileVersionInfoSizeW", "GetFileVersionInfoW",
    "VerFindFileA", "VerFindFileW", "VerInstallFileA", "VerInstallFileW",
    "VerLanguageNameA", "VerLanguageNameW", "VerQueryValueA", "VerQueryValueW",
}
pe = pefile.PE(sys.argv[1])
found = {e.name.decode() for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
missing = expected - found
if missing:
    print("    MISSING: " + ", ".join(sorted(missing)))
    sys.exit(1)
if pe.FILE_HEADER.Machine != 0x8664:
    print(f"    WRONG MACHINE: {pe.FILE_HEADER.Machine:#x}, expected 0x8664")
    sys.exit(1)
print(f"    OK - all 17 present, x86-64, {len(found)} exports total")
PY

if [[ $want_managed -eq 1 ]]; then
    echo "==> building Modrix.Loader (net6.0)"
    dotnet build -c Release "$MANAGED_DIR/Modrix.Loader.csproj"
    cp "$MANAGED_DIR/bin/Release/net6.0/Modrix.Loader.dll" "$DIST/_Modrix/net6/"
    echo "==> staged $DIST/_Modrix/net6/Modrix.Loader.dll"
fi

if [[ -n "$coreclr_src" ]]; then
    [[ -f "$coreclr_src/coreclr.dll" ]] || {
        echo "no coreclr.dll under $coreclr_src - point --coreclr at mini-coreclr's win-x64 folder" >&2
        exit 1
    }
    echo "==> copying the vendored runtime"
    cp -r "$coreclr_src/." "$DIST/_Modrix/dotnet/"
    echo "==> staged $(find "$DIST/_Modrix/dotnet" -type f | wc -l) runtime files"
fi

echo
echo "dist tree ready. It is NOT installed anywhere - see README.md, 'Installing'."
find "$DIST" -maxdepth 3 -mindepth 1 -not -path '*/dotnet/*' | sort
