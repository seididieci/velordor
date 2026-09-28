#!/usr/bin/env bash
# Build del tool host `arca` (Fase 54, P5) e creazione di `arca.img`.
#
# Il tool e' un binario HOST (std), ma il `.cargo/config.toml` della radice
# forza `target = x86_64-unknown-none` + `build-std` per tutti i crate del
# repo. Il config discovery di Cargo parte dalla CWD (non dal manifest), quindi
# si compila da una directory neutra con `--manifest-path`: il tool prende la
# std del toolchain, i crate bare-metal restano col loro config.
#
# Uso: scripts/arca-tool.sh [out_img]   (default userland/disk/arca.img)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/userland/disk/arca.img}"
TOOL_BIN="${ARCA_TOOL_BIN:-$ROOT/build-meta/arca}"

mkdir -p "$(dirname "$TOOL_BIN")" "$(dirname "$OUT")"
( cd "${TMPDIR:-/tmp}" && cargo build --release --manifest-path "$ROOT/tools/arca/Cargo.toml" )
cp "$ROOT/tools/arca/target/release/arca" "$TOOL_BIN"
"$TOOL_BIN" create "$OUT" --uuid 4152434100000001
echo "[arca] tool in $TOOL_BIN, immagine $OUT"
