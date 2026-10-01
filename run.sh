#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "[run] Scheduler: RT a 32 priorita' + CBS"

# Build dei binari userspace (userland/ = servizi utente) e della test suite
# (testland/) in pipeline separate, poi il kernel che li embedda entrambi
# (user_binary.rs via include_bytes!).
./scripts/build-userland.sh
./scripts/build-tests.sh

# Diagnostica scheduler/IRQ (feature `sched_debug`): snapshot ready-mask ogni
# 100 tick + log IRQ1. Spenta di default (log puliti); accesa con
# SCHED_DEBUG=1. run-tests.sh la imposta sempre.
if [ "${SCHED_DEBUG:-0}" = "1" ]; then
    echo "[run] sched_debug ATTIVO (log scheduler + IRQ1)"
    cargo build --release --features sched_debug
else
    cargo build --release
fi

# Immagini disco FAT32 (Fase 9.2 + 16d): generate a ogni run.
# fat.img = disco di boot (UUID 4F4C4556, label VELORDO, montato a /fat);
# fat2.img = secondo disco (UUID e label diversi + MARKER.TXT) per i test di
# identità stabile (t36) e il reorder (SWAP_DRIVES=1 inverte l'ordine IDE:
# le lettere sdX si scambiano, UUID=/LABEL= restano validi).
python3 scripts/mkfat.py userland/fs/fat.img
python3 scripts/mkfat.py userland/fs/fat2.img --serial C0FFEE01 --label SECOND --marker "second disk marker"

# Servizi da disco (Fase 21): /bin + /test iniettati via script condiviso
# (stesso usato da test-shell.py) DOPO mkfat. Fail-loud.
bash scripts/inject-bins.sh

# ArcaFS (Fase 54, P5): build del tool host + `arca.img` (superblock LBA0 +
# shadow). Sempre generata (economica); il terzo drive si aggiunge solo con
# ARCA_IMG=1, cosi' il boot default resta a due dischi (sda/sdb stabili per
# t32/t36).
./scripts/arca-tool.sh

# Fase 55, Parte 4: crea arca-part.img (MBR + partizione con superblock ArcaFS
# a offset LBA63) e arca-gpt.img (GPT + partizione ArcaFS a LBA64) quando
# ARCA_IMG=1. Servono per testare il mount in partizione MBR e GPT invece
# che su whole-disk.

# Fase 2 (root su volume): assegna UUID distinti alle derivate (MBR/GPT) per
# evitare collisioni con arca.img — il kernel sceglie la root per UUID (mai
# per lettera/scan). Le derivate sono copie di blocchi del volume originale:
# NON vanno seedate a loro volta.
if [ "${ARCA_IMG:-0}" = "1" ]; then
    ./scripts/build-arca-part.sh
    python3 ./scripts/build-arca-gpt.sh
    # UUID distinti per partizione MBR (LBA63) e GPT (LBA64).
    ARCA_TOOL_BIN="${ARCA_TOOL_BIN:-$ROOT/build-meta/arca}"
    "$ARCA_TOOL_BIN" uuid userland/disk/arca-part.img 4152434100000002 --lba 63
    "$ARCA_TOOL_BIN" uuid userland/disk/arca-gpt.img 4152434100000003 --lba 64
fi

KERNEL=target/x86_64-unknown-none/release/velord
DISPLAY="${RUN_DISPLAY:-none}"   # RUN_DISPLAY=gtk per vedere la VGA in locale

# Boot diretto via protocollo PVH (ELF64 + nota XEN_ELFNOTE_PHYS32_ENTRY):
# QEMU carica il kernel e trasferisce il controllo in protected mode 32-bit.
# Due drive IDE (Fase 16d): block li enumera sda,sdb in ordine di probe
# e cardo monta a /fat per UUID (mai per lettera).
if [ "${SWAP_DRIVES:-0}" = "1" ]; then
    DRIVES="-drive file=userland/fs/fat2.img,format=raw,if=ide -drive file=userland/fs/fat.img,format=raw,if=ide"
else
    DRIVES="-drive file=userland/fs/fat.img,format=raw,if=ide -drive file=userland/fs/fat2.img,format=raw,if=ide"
fi
# Terzo/quarto drive ArcaFS opt-in (Fase 54, P5; Fase 55, Parte 4: partizioni
# MBR su arca-part.img e GPT su arca-gpt.img = secondary master/slave).
# La suite li cerca via magic, mai per lettera (sda/sdb restano i due FAT).
if [ "${ARCA_IMG:-0}" = "1" ]; then
    DRIVES="$DRIVES -drive file=userland/disk/arca.img,format=raw,if=ide"
    DRIVES="$DRIVES -drive file=userland/disk/arca-part.img,format=raw,if=ide"
fi
# Fase 2 (root su volume): passa la cmdline `-append` a QEMU. Il kernel legge
# `hvm_start_info.cmdline_paddr` al boot e lo rende a userland via SYS_BOOT_
# CMDLINE (53). cardo parse `root=UUID=<8hex>` dal cmdline: monta il primo
# volume ArcaFS con quell'uuid come `/`, ramfs solo su `/tmp`. Se l'uuid non
# esiste o il parametro e' assente → kernel panic loud.
if [ -f userland/disk/arca.img ]; then
    APPEND="root=UUID=4152434100000001"
else
    APPEND=""
fi
# shellcheck disable=SC2086
exec qemu-system-x86_64 \
    -m 256M \
    -display "$DISPLAY" \
    -serial stdio \
    -no-reboot \
    -kernel "$KERNEL" \
    $DRIVES \
    $( [ -n "$APPEND" ] && echo "-append $APPEND" ) \
    "$@"
