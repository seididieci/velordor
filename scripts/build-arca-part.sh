#!/usr/bin/env bash
# Crea un'immagine MBR con superblock ArcaFS scritto a offset partizione LBA63.
#
# Uso: scripts/build-arca-part.sh [out_img]   (default userland/disk/arca-part.img)
# Requisiti: dd, mkfs.vfat (opzionale per formattare la partizione).
#
# Produce un disco con:
#   - MBR valido con prima partizione a LBA63 (offset 32256 B)
#   - Superblock ArcaFS scritto all'inizio della partizione (LBA63)

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/userland/disk/arca-part.img}"
ARCA_IMG_FILE="$ROOT/userland/disk/arca.img"

# Verifica che arca.img esista (contiene il superblock da copiare)
if [ ! -f "$ARCA_IMG_FILE" ]; then
    echo "[arca-part] errore: $ARCA_IMG_FILE non trovato (creare prima con scripts/arca-tool.sh)"
    exit 1
fi

OUT_DIR="$(dirname "$OUT")"
mkdir -p "$OUT_DIR"

# 1. Crea immagine vuota 32 MiB
dd if=/dev/zero of="$OUT" bs=1M count=32 2>/dev/null

# 2. Scrivi MBR manualmente (512 byte).
#    Struttura MBR:
#      [0x000..0x1BD] Bootloader vuoto (440 B)
#      [0x1BE..0x1FD] Tabella partizioni (64 B, 4 voci da 16 B)
#      [0x1FE..0x1FF] Signature (2 B = 0xAA55)
#
#    Voce partizione primaria (16 B a offset 0x1BE):
#      [0] boot flag (0x80 = active)
#      [1-3] CHI start (CHS: testa=1, settore=1, cilindro=1)
#      [4] tipo (0x83 = Linux)
#      [5-7] CHI end (stesso)
#      [8-11] LBA start LE u32 (63)
#      [12-15] settori LE u32 (65472 = 32MB/512 - 63)

{
    # Bootloader vuoto (440 byte a zero)
    dd if=/dev/zero bs=1 count=440 2>/dev/null

    # Tabella partizioni: prima voce + padding fino a offset 510
    printf '\x80\x01\x01\x00\x83\x01\x01\x00'   # boot, CHI start, tipo=Linux, CHI end
    printf '\x3f\x00\x00\x00'                   # LBA start = 63 (LE u32)
    printf '\xc0\xff\x00\x00'                   # settori = 65472 (LE u32)
    dd if=/dev/zero bs=1 count=48 2>/dev/null   # padding: 48 byte a zero (fino a offset 510)

    # Signature MBR (2 byte)
    printf '\x55\xaa'
} > /tmp/mbr.bin

# Sovrascrive i primi 512 byte con il MBR
dd if=/tmp/mbr.bin of="$OUT" bs=512 count=1 conv=notrunc 2>/dev/null
rm -f /tmp/mbr.bin

# 3. Scrivi superblock ArcaFS all'inizio della partizione (LBA63).
dd if="$ARCA_IMG_FILE" of="$OUT" bs=512 seek=63 count=1 conv=notrunc 2>/dev/null

echo "[arca-part] creata $OUT (MBR + ArcaFS a offset LBA63)"
