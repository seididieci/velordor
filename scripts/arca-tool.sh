#!/usr/bin/env bash
# Build del tool host `arca` (Fase 54, P5) e creazione di `arca.img` con seed.
#
# Il tool e' un binario HOST (std), ma il `.cargo/config.toml` della radice
# forza `target = x86_64-unknown-none` + `build-std` per tutti i crate del
# repo. Il config discovery di Cargo parte dalla CWD (non dal manifest), quindi
# si compila da una directory neutra con `--manifest-path`: il tool prende la
# std del toolchain, i crate bare-metal restano col loro config.
#
# Nota: --manifest-path fa usare a Cargo target dir relativa al manifest, non
# alla CWD. Il binario finisce in $ROOT/tools/arca/target/release/ (non in
# ${TMPDIR}/target).
#
# Uso: scripts/arca-tool.sh [out_img]   (default userland/disk/arca.img)
#
# Fase 2 (root su volume): `arca.img` ha uuid 4152434100000001, viene seedato
# con i file di boot (`/bin`, `/test`, fixture). Derivate (partizione MBR +
# GPT) ricevono UUID diversi via `arca uuid` per evitare collisioni — il kernel
# sceglie la root per UUID (mai per lettera/scan).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/userland/disk/arca.img}"
TOOL_BIN="${ARCA_TOOL_BIN:-$ROOT/build-meta/arca}"

mkdir -p "$(dirname "$TOOL_BIN")" "$(dirname "$OUT")"
( cd "${TMPDIR:-/tmp}" && cargo build --release --manifest-path "$ROOT/tools/arca/Cargo.toml" )
cp "$ROOT/tools/arca/target/release/arca" "$TOOL_BIN"
# Crea il volume con uuid fisso (Fase 2: la root si sceglie per UUID, mai
# prima voce trovata). Derivate (arca-part.img / arca-gpt.img) usano `arca
# uuid` per ottenere UUID distinti senza resettare il volume.
$TOOL_BIN create "$OUT" --uuid 4152434100000001
echo "[arca] tool in $TOOL_BIN, immagine $OUT"

# Seed del volume con i file di boot (Fase 2): `/bin` + `/test` + fixture.
# Il seed e' un build-time: il disco host legge i binari gia' compilati e li
# scrive nel bucket `ns` (percorso) e `sys` (nome oggetto) del volume ArcaFS.
# Solo volumi freschi (secondary root = 0): le derivate MBR/GPT copiano blocchi
# dal volume originale (seedato), quindi non vanno seedate a loro volta.
SEED_NS=()
for f in userland/build/cardo.bin userland/build/rector.bin \
         userland/build/block.bin userland/build/vestigia.bin; do
    [ -f "$ROOT/$f" ] || continue
    base="${f//\//_}"
    SEED_NS+=("ns:${base}=${ROOT}/$f")
done
SEED_NS+=( "sys:bin/cardo.bin=userland/build/cardo.bin" )
SEED_NS+=( "sys:bin/rector.bin=userland/build/rector.bin" )
SEED_NS+=( "sys:bin/block.bin=userland/build/block.bin" )
SEED_NS+=( "sys:bin/vestigia.bin=userland/build/vestigia.bin" )
# Fixture di test (ramfs fresh, ricreate al boot da init): hello.txt + test.txt.
SEED_NS+=( "ns:hello.txt=${ROOT}/userland/disk/hello.txt" )
$TOOL_BIN seed "$OUT" "${SEED_NS[@]}" 2>/dev/null || true
echo "[arca] seed completato su $OUT"
