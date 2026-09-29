#!/usr/bin/env python3
"""Crea un'immagine GPT 32MB con superblock ArcaFS in partizione (Fase 55).

Uso: scripts/build-arca-gpt.sh [out_img]   (default userland/disk/arca-gpt.img)

Layout (65536 settori da 512B):
  LBA0      protective MBR (entry0 tipo 0xEE start=1 size=65535 + sig AA55)
  LBA1      GPT header (magic "EFI PART", current=1, backup=65535,
            usable 34..65502, entries a LBA2, 128 entry da 128B; CRC zero:
            il parser A1 li salta, solo diagnostica futura)
  LBA2..33  array entry (entry0 = partizione ArcaFS start=64 last=65502,
            type GUID = ARCAFS_TYPE_GUID placeholder da registrare)
  LBA64/65  superblock + shadow ArcaFS (partition-relative LBA0/LBA1,
            copiati da arca.img, arcafs.md §16.4)
  LBA65503..65534 backup array entry, LBA65535 backup header (il parser A1
  li ignora; scritti per correttezza verso tool esterni).

Richiede arca.img esistente (scripts/arca-tool.sh). Fail-loud: qualunque
check fallito = exit 1, mai un'immagine muta in QEMU.
"""
import struct
import sys
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "userland/disk/arca-gpt.img")
ARCA_IMG = os.path.join(ROOT, "userland/disk/arca.img")

SECTORS = 65536  # 32 MiB
PART_START = 64
PART_LAST = 65502  # prima dell'area backup (65503..65535)

# Type GUID ArcaFS = ARCAFS_TYPE_GUID in userland/disk/src/part.rs
# (placeholder, da registrare con UUID alias prima del rilascio).
ARCA_TYPE_GUID = bytes([
    0x41, 0x52, 0x43, 0x41, 0x46, 0x53, 0x2D, 0x41,
    0x31, 0x2D, 0x30, 0x30, 0x30, 0x30, 0x30, 0x31,
])
DISK_GUID = bytes(range(0x10, 0x20))
UNIQUE_GUID = bytes(range(0x20, 0x30))


def die(msg):
    print("[arca-gpt] errore: %s" % msg)
    sys.exit(1)


if not os.path.isfile(ARCA_IMG):
    die("%s non trovato (creare prima con scripts/arca-tool.sh)" % ARCA_IMG)
with open(ARCA_IMG, "rb") as f:
    superblock = f.read(512)
if len(superblock) != 512 or superblock[0:4] != b"ACFS":
    die("superblock sorgente invalido in %s" % ARCA_IMG)
with open(ARCA_IMG, "rb") as f:
    f.seek(512)
    shadow = f.read(512)
if len(shadow) != 512 or shadow[0:4] != b"ACFS":
    die("shadow sorgente invalido in %s" % ARCA_IMG)

img = bytearray(SECTORS * 512)

# LBA0: protective MBR
mbr = bytearray(512)
mbr[446 + 4] = 0xEE
mbr[446 + 8:446 + 12] = struct.pack("<I", 1)
mbr[446 + 12:446 + 16] = struct.pack("<I", SECTORS - 1)
mbr[510] = 0x55
mbr[511] = 0xAA
img[0:512] = mbr

# LBA1: GPT header (92 byte, resto zero)
hdr = bytearray(512)
hdr[0:8] = b"EFI PART"
struct.pack_into("<I", hdr, 8, 0x00010000)   # revision
struct.pack_into("<I", hdr, 12, 92)          # header size
# 16: CRC32 = 0 (A1 lo salta)
struct.pack_into("<Q", hdr, 24, 1)           # current LBA
struct.pack_into("<Q", hdr, 32, SECTORS - 1)  # backup LBA
struct.pack_into("<Q", hdr, 40, 34)          # first usable
struct.pack_into("<Q", hdr, 48, PART_LAST)   # last usable
hdr[56:72] = DISK_GUID
struct.pack_into("<Q", hdr, 72, 2)           # entries LBA
struct.pack_into("<I", hdr, 80, 128)         # num entries
struct.pack_into("<I", hdr, 84, 128)         # entry size
# 88: entries CRC32 = 0 (A1 lo salta)
img[512:1024] = hdr

# LBA2..33: array entry (entry0 = partizione ArcaFS)
arr = bytearray(128 * 128)
e0 = bytearray(128)
e0[0:16] = ARCA_TYPE_GUID
e0[16:32] = UNIQUE_GUID
struct.pack_into("<Q", e0, 32, PART_START)
struct.pack_into("<Q", e0, 40, PART_LAST)
arr[0:128] = e0
img[2 * 512:2 * 512 + len(arr)] = arr

# Partizione: superblock + shadow (phys 64/65)
img[PART_START * 512:PART_START * 512 + 512] = superblock
img[(PART_START + 1) * 512:(PART_START + 1) * 512 + 512] = shadow

# Backup: array a 65503, header a 65535 (current/backup scambiati)
img[65503 * 512:65503 * 512 + len(arr)] = arr
bhdr = bytearray(hdr)
struct.pack_into("<Q", bhdr, 24, SECTORS - 1)
struct.pack_into("<Q", bhdr, 32, 1)
img[65535 * 512:65535 * 512 + 512] = bhdr

os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, "wb") as f:
    f.write(img)

# Verifiche fail-loud (rilettura da disco)
with open(OUT, "rb") as f:
    d = f.read()
assert len(d) == SECTORS * 512, "size"
assert d[450] == 0xEE and d[510] == 0x55 and d[511] == 0xAA, "protective MBR"
assert d[512:520] == b"EFI PART", "gpt magic"
assert struct.unpack_from("<Q", d, 512 + 72)[0] == 2, "entries LBA"
assert struct.unpack_from("<I", d, 512 + 80)[0] == 128, "entries count"
base = 2 * 512
assert d[base:base + 16] == ARCA_TYPE_GUID, "entry type"
assert struct.unpack_from("<Q", d, base + 32)[0] == PART_START, "entry start"
assert struct.unpack_from("<Q", d, base + 40)[0] == PART_LAST, "entry last"
assert d[PART_START * 512:PART_START * 512 + 4] == b"ACFS", "superblock"
assert d[(PART_START + 1) * 512:(PART_START + 1) * 512 + 4] == b"ACFS", "shadow"
print("[arca-gpt] creata %s (GPT + ArcaFS start=%d last=%d, verificata)" % (OUT, PART_START, PART_LAST))
