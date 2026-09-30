#!/usr/bin/env bash
# Gate di regressione Velordor: boot con la test suite completa.
#
# `./run.sh` di default e' produzione (init salta i test, shell subito).
# Questo wrapper imposta RUN_TESTS=1 (init compilato con --no-default-features
# ed esegue usertestfs/usertestfat/usertests/testsarca in sequenza prima della
# shell) e rimanda a run.sh. Righe attese + zero FAIL/PANIC/FAULT:
#   [testfs] PASS 5/5
#   [testfat] PASS 7/7
#   [usertests] PASS 58/58
#   [testsarca] PASS 40/40
set -euo pipefail
cd "$(dirname "$0")"
# Diagnostica scheduler/IRQ attiva nei run di test (feature `sched_debug`).
# ARCA_IMG=1: terzo e quarto drive ArcaFS (MBR + GPT in partizione) per
# testsarca (il core resta PASS anche se assenti; nel gate sono presenti).
RUN_TESTS=1 SCHED_DEBUG=1 ARCA_IMG=1 exec ./run.sh "$@"
