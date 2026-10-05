# ROADMAP Velordo

Sorgente unica di **stato** (fasi completate) e **futuro** (Pianificate +
Parcheggiate). La storia dettagliata per fase (decisioni, bug trovati,
lezioni, validazioni) vive in `docs/src/14-cronologia-fasi.md`; i gate
citati li' sono snapshot storici — il gate corrente e' in
`docs/src/11-testing.md`.

Orizzonte (dichiarato, non roadmap): self-hosting — un Velordo capace di
ricompilare se stesso. Ordina le priorita' (storage veloce prima, servizi
fuori dal kernel poi, personalita' per software reale dopo) senza pianificare
nulla da solo.

## Stato (completate 1-55)

Gate: `[testfs] PASS 5/5` + `[testfat] PASS 7/7` + `[testsarca] PASS 50/50` +
`[posixtests] PASS 4/4` + `[usertests] PASS 54/54` + shell (10/10 fasi), zero FAIL/PANIC/FAULT.

| Fase | Descrizione | Stato |
|------|-------------|-------|
| 1 | Bare metal Hello World (VGA) + boot PVH | ✅ Completata |
| 2 | Memory Map (PVH) + GDT/IDT | ✅ Completata |
| 3 | Interrupt hardware (PIC/PIT/kbd) | ✅ Completata |
| 4 | Frame allocator + heap kernel (dinamico; direct map 64G da Fase 27) | ✅ Completata |
| 5 | Processi + scheduler preemptive | ✅ Completata |
| 6 | User mode (ring 3) + syscall | ✅ Completata |
| 7 | IPC sincrona send/recv ⭐ | ✅ Completata |
| 8 | init + console server | ✅ Completata |
| 9 | File system server via IPC (ramfs/FAT32/devfs/shell, Fase 9.1-9.6) | ✅ Completata |
| 10 | IPC optimizations (ring msg_queue, bitmask pick_next, SPSC ring FS) | ✅ Completata |
| 11 | Scheduler RT a 32 priorita' + CBS (bandwidth reservation) | ✅ Completata |
| 12 | IPC per nome — registry + channel nel kernel (ADR-0008) | ✅ Completata |
| 13 | IPC asincrono: send/recv non bloccanti, request-id (ADR-0009) | ✅ Completata |
| 14 | Cleanup processi: exit/kill, notifica al parent, slot a generazioni (ADR-0010) | ✅ Completata |
| 15 | Keyboard + Terminal server in userspace (sgancio tastiera/VGA) | ✅ Completata |
| 16 | Disk/ATA driver server in userspace (sgancio ATA/FS) | ✅ Completata |
| 17 | Diritti per-canale lato server (capability su IPC) | ✅ Completata |
| 18 | Shell + utility utente | ✅ Completata |
| 19 | Introspezione (`ps`) + metadati (`stat`) | ✅ Completata |
| 20 | FAT32 scrivibile (persistenza, ADR-0016) | ✅ Completata |
| 21 | Servizi caricati da disco via `spawn_image` (ADR-0017) | ✅ Completata |
| 22 | Detach dalla cascata di morte (emendamento ADR-0010 §6) | ✅ Completata |
| 23/24/25 | Baseline + ottimizzazioni throughput (PIO multi-settore, cache settoriale write-through) | ✅ Completata |
| 26 | `async`/`await` in `libr` sopra IPC asincrona (ADR-0019, 4 passi) | ✅ Completata |
| 27 | Higher-half kernel + direct map (ADR-0020: 27.1/27.2/27.3) | ✅ Completata |
| 28 | `mmap` anonimo nel basso canonico (payoff higher-half) | ✅ Completata |
| 29 | Protezioni di memoria (`mprotect`/NX, fault→kill del processo) | ✅ Completata |
| 30 | Memoria condivisa tra processi (`shm_create`/`shm_map`) | ✅ Completata |
| 31 | Loader ELF per-segmento (W^X del binario, ADR-0021) | ✅ Completata |
| 32 | Shared text ELF (segmenti immutabili condivisi, ADR-0022) | ✅ Completata |
| 33 | Infrastruttura COW (frame refcount + COW fault, ADR-0023) | ✅ Completata |
| 34 | `fork` — COW dell'address space (ADR-0024) | ✅ Completata |
| 35 | Hardening (threat model + cancelli kernel, ADR-0025/0026) | ✅ Completata |
| 36 | Identità misurata (hash nel PCB + manifest + policy su identità, ADR-0027) | ✅ Completata |
| 37 | `exec` in-place + shell che lancia programmi (run/jobs/wait, ADR-0028) | ✅ Completata |
| 38 | ATA DMA + IRQ (ADR-0029) | ✅ Completata |
| 39 | Fondamenta posix: registry 8→16 + `Service::Posix`, `libr::posix`, harness t53 (ADR-0030) | ✅ Completata |
| 40 | fd virtuali + redirect file (`> >> < 2> 2>&1`, `R_LSEEK`, `O_TRUNC`/`O_APPEND`, `R_DUP_*` modello B, errori tipati, t54) | ✅ Completata |
| 41 | Parser shell (quoting, `$VAR`, `; && \|\|`, glob) | ✅ Completata |
| 42 | Pipe + waitpid | ✅ Completata |
| 43 | Env/PATH/script (shebang, history; 43a ADR-0033 + 43b ADR-0034) | ✅ Completata |
| 44 | Job control + segnali (44a suspend/resume + fg/bg/Ctrl-Z, ADR-0035; 44b Ctrl-C selettivo + catch, ADR-0036) | ✅ Completata |
| 45 | Indurimento + chiusura posix (policy su identità + sandbox build, ADR-0037) | ✅ Completata |
| 46 | Provider trait filesystem (`LocalFs` + `LocalFsDyn`, `MountedFs::Local`, ADR-0038) | ✅ Completata |
| 47 | Wiring trait per ramfs (U1: open/read/write/readdir/stat/mkdir/delete) | ✅ Completata |
| 48 | Wiring trait per FAT32 (U2: `LocalFsDyn`, fix stat readonly) | ✅ Completata |
| 49 | Terreno pre-ArcaFS (T0: `AnyHandle`, mount-id stabili, `Source`+`fstype`, `Local` vivo, create/truncate nel trait) | ✅ Completata |
| 50 | Orologio P1 (`Service::Time`, `usertime` CMOS+`TIME_NOW`, `mtime` veri ramfs+FAT via trait, t38 esteso) | ✅ Completata |
| 51 | Vocabolario disco P2 (`DISK/R_DISK_LIST+INFO`, IDENTIFY estesa, relay cardo, `libr::disk_*`, t32 esteso, dati S1/S2) | ✅ Completata |
| 52 | Durabilita' P3 (`R_SYNC` modi+barriera, `RIGHTS_SYNC`, `R_STATVFS` nel trait, `SYS_MEMINFO`, contratto, t32/t37 estesi) | ✅ Completata |
| 53 | Misura bulk P4 (audit CAP, sweep 4K/16K/64K ramfs+FAT hot/cold in userbench, tabella costo-vs-dimensione) | ✅ Completata |
| 54 | Integrita' + attrezzi P5 (crate `blake2s`, `R_GET_HASH`, superblock ArcaFS, `negotiate()`→arcafs stub, `arca create` host, `arca list/stat` guest, testsarca 8/8) | ✅ Completata |
| 55 | ArcaFS A1+N0 (`R_OBJ_PUT/GET` + store in-memory, parser GPT per spec, mount MBR/GPT in partizione, `sys` seedato, init dual-mode + doppio pinning FNV/BLAKE2s, fallback FAT provato, testsarca 13/13, t58) | ✅ Completata |

## Pianificate

Voci con scope e vittoria dichiarati (non date).

- [ ] **ArcaFS/object-store** (dopo T0/Fase 49): filesystem avanzato stile
      btrfs/zfs — snapshot, niente partizionamento, layout adattivo per tipo
      di device (hdd/ssd/emmc), raid avanzato, monitoring (SMART), COW su
      disco; hook futuro per distribuzione nativa (ceph/glusterfs-like).
      Sessione A0 completata: bozza in `arcafs.md` (modello, indice,
      formato, API, mapping, diritti, quota, tool, multi-device, swap,
      vector, logging §15) + piano OS-first P1–P5 (§13). P1/50 chiusa
      (orologio), P2/51 chiusa (vocabolario disco + dati S1/S2), P3/52 chiusa
      (durabilita': contratto + barriera + sensori), P4/53 chiusa (misura
      bulk: tabella costo-vs-dimensione in `13-performance.md`), P5/54 chiusa
       (integrita' + attrezzi: BLAKE2s, content_hash, `arca create`/`list`,
       `negotiate()`→arcafs stub, testsarca 8/8). A1+N0/55 chiusa
       (object store in-memory, mount MBR/GPT in partizione, `sys`
       seedato, init dual-mode + pinning FNV/BLAKE2s, testsarca 13/13).
       Prossimo: stesura di dettaglio + ADR per A2, poi 56.
       Vittoria: spec scritta + primo mount.
- [x] **56 (A2)** (completata: 56.1 versioni in RAM 21/21; 56.2a casa `arcafs/`
       + formato on-disk + allocatore + `R_ARCA_DEBUG` 27/27; 56.2b B+tree COW +
       commit shadow+flip su disco, backend unico 32/32; 56.2c recovery/orphan-GC
       + tabella snapshot persistente + sys-dal-volume all'avvio 33/33): COW +
       snapshot/clone + GC + rollback/retention persistenti. Packing S1/S2 e
       `R_OBJ_MGET` (se 53 lo chiede) restano rinviati (i marker dir non
       servono piu': 56.3, emergenti + set RAM).
       Vittoria conseguita: rollback vero (persistente, sopravvive al kill);
       retention log implementabile (sblocco 57).
- [x] **56.3 (namespace POSIX su ArcaFS)** (completata, ADR-0042):
  dir emergenti (esistono ⟺ chiavi col prefisso nel bucket `ns`) + set
  transient in RAM per le `mkdir` (`mkdir` mai su disco, `rmdir`
  mai-esistita = errore, vuote perse al restart); vista POSIX in
  `userland/cardo` via motore globale a uuid combaciante (`ArcaWith`,
  mai dati altrui); `scan_prefix` nel B+tree; `R_SYNC GROUP` = commit;
  suite 40/40 → 50/50.
  Vittoria conseguita: file/dir su volume ArcaFS + intatti dopo kill.
- [x] **57 (L1) logging** (chiusa, ADR-0039; L0 cancellato senza implementarlo:
      la ragione — ArcaFS non esisteva — e' estinta): `vestigia`
      (`Service::Vestigia` = 10, embedded boot-TCB, spawn parallelo a block prima
      di fs) RAM-first con `LOG_FLUSH` di init (handshake FS + backdate/re-key
      pre-Time + riversamento incrementale + dual-write), bucket per identita'
      (`peer_info`, mai dichiarato), `!idx` per la latest post-restart, seal
      via snapshot, client `libr::log` su anelli condivisi (cancello leggero,
      mai handshake FS), regola non-POSIX. testsarca 33→40 (registrato,
      append+latest own-bucket, 1024B, rifiuti, seal+delete, stats con
      flush-proof, bounce+rewarm).
      Vittoria conseguita: log persistenti nativi con retention via snapshot+GC.
- [x] **58 Personalità POSIX separata** (58.1–58.6, ADR-0041): meccanismo
      neutro in `libs/civis` (ex `libs/libr`), personalità in
      `flavours/posix/{libr,server,shell,cli,tests}`. 58.1 (docs/ADR), 58.2
      (rename `libr`→`civis`), 58.3 (estrazione crate + inversione hook
      `entry!`/`route_out`/redir-magic), 58.4 (spostamento package in
      `flavours/posix/{server,shell,cli}`, output in `flavours/posix/build`),
      58.5 (suite `userposixtests` in `flavours/posix/tests`, gate
      `[posixtests] 4/4`, usertests 54/54) e 58.6 (chiusura docs) chiusi.
       Vittoria: `rg "posix::" libs/civis/src` = zero; una seconda personalità
       riusa `civis` invariato.
- [x] **Root su volume (Fase 2: C/A/D1/D2/E)** (chiusa, ADR-0043): ArcaFS
      come `/` scelto per UUID (`root=UUID=`, terzo drive sempre attaccato),
      ramfs solo su `/tmp`, `/fat` montato per dati (niente fallback).
      C: seed build-time (volume 32MiB, 90 voci, fail-loud; derivate MBR/GPT
      adeguate). A: mount root catch-all (`target ""`, `/dev` escluso),
      motore legato al volume root, parità errori IsDir/Exists su Arca.
      D1: init carica i servizi per object_id (bucket `vela` = driver per
      `io_ranges`, `sys` = servizi, `usr` reserved; tree `/bin`, `/bin/posix`,
      `/usr/bin`; PATH `/usr/bin:/bin`), fallback FAT rimosso. D2: suite via
      oggetti `tst`, helper via path `/test` (dogfood VFS). E: shell suite
      verde (harness col drive root + overlay arca per `--jobs`), testsarca
      50/50 pieni (drive GPT per 12-13, 22-32 su `sdc` root in bucket isolati,
      41-50 su mount root esplicito; parser MBR fuori dal gate, vedi
      `11-testing.md`).
      Vittoria conseguita: gate 5/5+7/7+50/50+4/4+54/54 + shell 10/10, zero
      fallback FAT in init e nella shell.
- [ ] **59+ (A3–A8, V1, B1)** (dopo 56, come da `arcafs.md` §13): quota,
      ABAC engine, device-awareness, RAID, tool completo, rete; servizio
      vettoriale e backend VM/block fuori dal FS, mai dentro.
      Vittoria: una fase alla volta, ciascuna col suo gate.

## Parcheggiate

Idee senza trigger (non date, solo su pressione reale).

- [ ] audio AC97+CBS (primo client servizio PCI, chiude ADR-0007 davvero)
- [ ] server-run async block (quando l'overlap DMA lo richiede)
- [ ] Strato 3 credenziali
- [ ] ext2/ATAPI/write-back/read-ahead/`DISK_STATS`/generazioni PID/thread
      (solo su pressione reale)
- [ ] OOM-kill a load (negativa ADR-0028)
