#!/usr/bin/env bash
# Wrap the wasm32-unknown-unknown build of dutchman-plugin as a SolidCraft plugin package:
#   scripts/package-plugin.sh <dutchman_plugin.wasm> <output folder> [<version>]
# writes <output folder>/dutchman.wasm, plugin.json, icons/ and dutchman-plugin.zip. With a version
# (a release's X.Y.Z) the manifest says that version and the zip is dutchman-plugin-<version>.zip,
# with a .sha256 beside it.
#
# SolidCraft links only craft:plugin interfaces. `zip` turns on getrandom's JavaScript
# backend for every wasm32 target, so the core module also imports a few wasm-bindgen
# functions (randomness for archive extraction and encryption, which Dutchman never calls).
# Each such import is bound to a stub that traps, and the script fails unless the finished
# component imports craft:plugin interfaces only.
#
# The core module is shrunk with binaryen's wasm-opt first (PLUGIN_WASM_OPT_ARGS), because
# SolidCraft reads and hashes the whole component when it lists plugins. Release builds pin
# binaryen version_123 (docs/plugin.md). WASM_OPT=<path> names another binary;
# PLUGIN_SKIP_WASM_OPT=1 skips the step (then also raise PLUGIN_WASM_MAX_BYTES).
set -euo pipefail
if [ $# -lt 2 ] || [ $# -gt 3 ]; then
    echo "usage: $0 <dutchman_plugin.wasm> <output folder> [<version>]" >&2
    exit 2
fi
core=$1
out=$2
version=${3:-}
wasm_tools=${WASM_TOOLS:-wasm-tools}
wasm_opt=${WASM_OPT:-wasm-opt}
root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out"
stubs=$(mktemp -d "$out/.stubs.XXXXXX")
trap 'rm -rf "$stubs" "${optimized:-}"' EXIT
if [ "${PLUGIN_SKIP_WASM_OPT:-}" != 1 ]; then
    if ! command -v "$wasm_opt" >/dev/null 2>&1; then
        echo "wasm-opt (binaryen version_123) is needed to shrink the plugin; set WASM_OPT, or PLUGIN_SKIP_WASM_OPT=1 to skip" >&2
        exit 2
    fi
    optimized=$(mktemp "$out/.core.XXXXXX")
    # Only the features rustc emits for wasm32-unknown-unknown; --all would add GC types that
    # the host may not accept.
    "$wasm_opt" -Oz --enable-bulk-memory --enable-bulk-memory-opt --enable-sign-ext \
        --enable-nontrapping-float-to-int --enable-mutable-globals --enable-multivalue \
        --enable-reference-types --strip-debug --strip-producers "$core" -o "$optimized"
    core=$optimized
fi

# Imports come before the first function body; stop printing there.
"$wasm_tools" print "$core" | awk '/^  \(func /{exit} /^  \(import "__wbindgen/{print}' >"$stubs/imports.txt" || true
adapt=()
for module in $(sed -nE 's/^  \(import "(__wbindgen[A-Za-z0-9_]*)".*/\1/p' "$stubs/imports.txt" | sort -u); do
    {
        echo "(module"
        grep -F "(import \"$module\" " "$stubs/imports.txt" | sed -nE \
            's/^  \(import "[^"]+" "([^"]+)" \(func [^(]*(\(;[0-9]+;\) )?\(type [0-9]+\) ?(.*)\)\)$/  (func (export "\1") \3 unreachable)/p'
        echo ")"
    } >"$stubs/$module.wat"
    "$wasm_tools" parse "$stubs/$module.wat" -o "$stubs/$module.wasm"
    adapt+=(--adapt "$module=$stubs/$module.wasm")
done

"$wasm_tools" component new "$core" "${adapt[@]}" -o "$out/dutchman.wasm"
imports=$("$wasm_tools" component wit "$out/dutchman.wasm" | sed -nE 's/^ *import ([^;]+);/\1/p')
echo "$imports"
if echo "$imports" | grep -qv '^craft:plugin/'; then
    echo "The component imports something besides craft:plugin interfaces" >&2
    exit 1
fi
# SolidCraft reads and hashes this module when it lists plugins and compiles it on a file's
# first open, so its size is a budget: 0.3.0 was 55.7 MB, and with wasm-opt it is 45.3 MB; this
# limit is about 1.1x that. Raise PLUGIN_WASM_MAX_BYTES only on purpose.
max_bytes=${PLUGIN_WASM_MAX_BYTES:-50000000}
size=$(wc -c <"$out/dutchman.wasm")
if [ "$size" -gt "$max_bytes" ]; then
    echo "dutchman.wasm is $size bytes, over the $max_bytes byte budget (PLUGIN_WASM_MAX_BYTES); SolidCraft compiles it on a file's first open" >&2
    exit 1
fi
zip_name=dutchman-plugin.zip
if [ -n "$version" ]; then
    case "$version" in
        *[!0-9.]* | .* | *. | *..*) echo "The version must look like X.Y.Z, not $version" >&2; exit 2 ;;
    esac
    python3 - "$root/crates/dutchman-plugin/plugin.json" "$out/plugin.json" "$version" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1]))
manifest["version"] = sys.argv[3]
with open(sys.argv[2], "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY
    zip_name=dutchman-plugin-$version.zip
else
    cp "$root/crates/dutchman-plugin/plugin.json" "$out/plugin.json"
fi
rm -f "$out/$zip_name"
mkdir -p "$out/icons"
cp "$root/crates/dutchman-plugin/icons/dutchman.svg" "$out/icons/dutchman.svg"
(cd "$out" && zip -q -X "$zip_name" plugin.json dutchman.wasm icons/dutchman.svg)
if [ -n "$version" ]; then
    (cd "$out" && sha256sum "$zip_name" > "$zip_name.sha256")
fi
wc -c "$out/dutchman.wasm"
