# ADR-0045: Self-hosting rustc — survey S0 (requisiti + gap + scala)

## Status

Accepted (survey; vincola S1…Sn ma non implementa nulla)

## Context

Orizzonte: Velordo che ricompila se stesso. Il FS c'è (root ArcaFS
persistente + quota + snapshot, A3). S0 chiede: cosa serve ancora per far
girare un rustc nativo, e tcc a cosa serve davvero. Strategia fissata:
**cross-compilare tutto su host** (LLVM, rustc + PAL velordo, linker) e far
girare su Velordo solo l'ultimo miglio (rustc che compila). Costruire rustc
su Velordo è fuori portata per disegno (servirebbero nativi Python, cmake,
C++, make/ninja, git, curl + GB di RAM per LLVM).

Misure host (S0.5, rustc nightly, hello-world): ~0.05–0.5 s, **~85 MB RSS**,
toolchain 1.9 GB (lib 360 MB, solo sysroot utile ~200–400 MB). QEMU gira a
256M con volumi da 32MiB: entrambi sottodimensionati per rustc (serve ordine
di 1 GB disco + qualche centinaio di MB RAM anche solo per hello; il kernel
vero vuole GB).

## Decision

### S0.3 — il nodo thread: rustc vuole thread OS, sempre

`strace -f` su `rustc hello.rs` (anche `-C codegen-units=1`): **32 clone**
(rayon, LLVM, pool istanziati eager — nessun flag li azzera; il frontend è
single-thread di default ma il runtime spawna comunque). Quindi std con
`no_threads` stile uefi NON basta a far girare rustc: serve la **tsoa dei
thread kernel** (clone con mm condiviso + TLS + parcheggio/futex) come epic
a sé (S-T), prerequisito duro di qualunque esecuzione di rustc. Corollario:
tcc (single-thread) gira senza S-T → prima vittoria toolchain eseguibile.

### S0.4 — linker: rust-lld dal sysroot, tcc come `cc` di bootstrap

La distribuzione Rust spedisce già `rust-lld` + binutils LLVM
(`lib/rustlib/<target>/bin/`): cross-compilata e copiata nel sysroot
nativo, con target-spec `linker: rust-lld` (pattern Xous), il linker è
risolto a costo zero. tcc resta utile come **`cc` di bootstrap** (piccoli
tool C nativi) e fallback, non come via per Rust.

### S0.1 — gap PAL (moduli `library/std/src/sys/pal` vs 45 syscall)

Precedenti: Xous (microkernel a messaggi, il più vicino a noi), uefi
(single-thread), hermit. Classificazione per far *girare* rustc (non per
costruirlo) + strato (regola: POSIX puro = flavour, il meccanismo solo se
inevitabile):

| Modulo PAL | Stato Velordo | Classe | Strato |
|---|---|---|---|
| alloc (GlobalAlloc) | heap civis ✓ | OK | — |
| args + env | argv+env via exec ✓ (43a) | OK | — |
| fs (open/read/write/close/seek/stat/mkdir/remove) | ✓ via cardo | OK | — |
| **rename** | assente | MUST | meccanismo (cardo + op) |
| **chdir/getcwd** | assenti (PCB senza cwd) | MUST | meccanismo (cwd nel PCB) + flavour |
| **symlink/readlink/link** | assenti (spec li rimanda al porting) | MUST | meccanismo (nodi cardo) + flavour |
| **chmod/umask/access** | assenti (spec: chmod non deve rompere i build) | MUST | flavour (projection ABAC, A4 aiuta) |
| fsync/file-lock | assenti | SHOULD | meccanismo, dopo i MUST |
| pipe/process (spawn/wait) | ✓ (fork/exec/wait, exit-notify) | OK | — |
| thread + TLS + futex | assenti (S0.3: dure) | MUST (S-T) | meccanismo (kernel) |
| time (wall) | ✓ (Service::Time) | OK | — |
| stdio | ✓ (redirect/stdio granite) | OK | — |
| net/socket | assenti | DEFER | build ermetici, niente rete per S1 |
| backtrace/dynamic-lib | assenti | DEFER | binari statici (loader attuale) |
| random | /dev слове? da verificare in S1 | SHOULD | meccanismo se manca |

rustc linkato **statico** (niente dynamic loader: il nostro carica solo
ELF statici); LLVM con thread inclusi (il cross-build li ha, il run li usa —
vedi S0.3).

### S0.2 — fabbisogni di run di rustc

Sysroot scoperto dal path dell'eseguibile (nessuna env obbligatoria se il
layout è preservato); temp via TMPDIR→volume capiente (la ramfs `/tmp` è
piccola); invocazione linker via fork/exec ✓; cwd (→ MUST sopra); env ✓.

### Scala S1…Sn (numeri assegnati qui)

- **S1**: MUST meccanismo+flavour senza thread (rename, chdir/getcwd,
  symlink, chmod-projection, volumi ≥1 GB, RAM QEMU multi-GB) + PAL velordo
  single-thread che fa girare programmi std semplici. Vittoria: hello std
  cross-compilato gira nativo.
- **S-T**: thread kernel + TLS + futex. Vittoria: `std::thread::spawn` nativo.
- **S2**: sysroot nativo (rustc + rust-lld cross-compilati) + `rustc hello`
  nativo. Vittoria: primo oggetto compilato *da* Velordo.
- **S3**: tcc nativo come `cc` + programma C non banale compilato nativo.
- **S4**: il kernel ricompilato da sé (self-hosting dichiarato).
- B1/swap resta differito (la RAM QEMU può crescere, lo swap no).

## Consequences

- Risposta a "tcc serve?": sì, ma come `cc` di bootstrap e prima vittoria
  senza thread — non come via per Rust (quella è PAL + S-T + rust-lld).
- A4 (ABAC) resta prima o parallela a S1: la projection chmod ci si appoggia.
- A8 (rete) esplicitamente fuori dalla scala self-hosting.
