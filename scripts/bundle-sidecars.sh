#!/usr/bin/env bash
set -euo pipefail

SIDECARS=(buzz-acp buzz-agent buzz-dev-mcp git-credential-nostr buzz)
HOST=$(rustc -vV | sed -n 's|host: ||p')
TARGET=${1:-$HOST}
PROFILE=${SIDECAR_PROFILE:-release}
BINARIES_DIR="desktop/src-tauri/binaries"

# When --target is passed explicitly to cargo (even if it matches the host),
# binaries land in target/<triple>/<profile>/. Without --target, they land in
# target/<profile>/. The script receives the target as $1 only when cargo was
# invoked with --target, so use the qualified path whenever $1 is set.
if [[ -n "${1:-}" ]]; then
    SRC_DIR="target/${TARGET}/${PROFILE}"
else
    SRC_DIR="target/${PROFILE}"
fi

# MSVC emits <name>.exe; Tauri's externalBin then expects binaries/<name>-<triple>.exe.
if [[ "$TARGET" == *windows* ]]; then
    EXE=".exe"
else
    EXE=""
fi

missing=()
for bin in "${SIDECARS[@]}"; do
    [[ -f "$SRC_DIR/${bin}${EXE}" ]] || missing+=("${bin}${EXE}")
done
if [[ ${#missing[@]} -gt 0 ]]; then
    echo "Error: missing sidecar binaries in $SRC_DIR: ${missing[*]}" >&2
    echo "Build the sidecar packages with the '${PROFILE}' profile first." >&2
    exit 1
fi

mkdir -p "$BINARIES_DIR"
for bin in "${SIDECARS[@]}"; do
    destination="$BINARIES_DIR/${bin}-${TARGET}${EXE}"
    cp "$SRC_DIR/${bin}${EXE}" "$destination"
    if [[ "$TARGET" != *windows* ]]; then
        chmod 0755 "$destination"
    fi
done
echo "Sidecars bundled for $TARGET from the $PROFILE profile"
