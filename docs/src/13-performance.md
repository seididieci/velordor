# Performance (Fasi 23–25 — baseline + ottimizzazioni; 38 — DMA/IRQ)

> Piattaforma di riferimento: **KVM** (`-accel kvm -cpu host`). I tempi TCG
> sono emulati e NON di riferimento. Orologio: TSC in ring 3 (CR4.TSD mai
> impostato), calibrato sul PIT via `civis::tsc_calibrate` (~4.45 GHz sul
> riferimento 23/24, ~1.6 GHz sull'host della campagna 25: confrontare solo
> misure dello STESSO host). Nessuna cache nel percorso dati fino a 24
> (25 aggiunge la cache settoriale, vedi sotto).

## Harness

- `testland/bench` (`userbench`, `/test/bench.bin`): 6 op end-to-end con
  warmup, stampa righe `[bench] <nome> iters=<n> cyc_op=<c> max_cyc=<m> kb_s=<k>`.
- `scripts/bench.sh` (`RUNS=3`, `TIMEOUT_S=300`): N boot KVM, fail-loud se il
  bench non completa (`DONE ok=1`). Esecuzione: `RUN_BENCH=1` compila init
  con feature `bench` (ortogonale a `skip_tests`): il bench gira dopo
  l'eventuale suite, prima della shell — **mai nel gate** di regressione.

## Baseline 23 (KVM, media 3 run, TSC ~4.45 GHz) → 24 (stessa base)

| Op | 23 cyc/op | 24 cyc/op | 24 latenza | 24 throughput |
|----|-----------|-----------|------------|---------------|
| `zero_1B` (2000× read 1 B `/dev/zero`) | ~8.2 K | ~8.3 K (invariato) | ~1.9 µs | — |
| `sda_512B_seq` (200× read 512 B `/dev/sda`) | ~5.43 M | ~5.45 M (invariato) | ~1.2 ms | ~409 KiB/s |
| `fat_small_orc` (500× open+read+close 25 B) | ~92.6 M | ~10.9 M (**8.5x**) | ~2.4 ms | ~10 KiB/s |
| `ramfs_4K_write` (100× write 4 KiB) | ~110 K | ~109 K (invariato) | ~25 µs | ~161 MiB/s |
| `ramfs_4K_read` (100× read 4 KiB) | ~62 K | ~61 K (invariato) | ~14 µs | ~289 MiB/s |
| `fat_4K_oow` (50× open+overwrite+close 4 KiB) | ~285 M | ~129 M (**2.2x**) | ~30 ms | ~135 KiB/s |

## 24 — cosa ha funzionato

- **24.1 (solo cardo, nessun cambio di protocollo)**: memo dell'ultimo
  settore FAT (invalidata a `set_fat_entry`, azzerata a ogni epoca) +
  letture a settori mirati (`read_file` per span, `read_dir` con stop al
  terminatore 0x00 invece dell'intero cluster). Da sola: small FAT 8.5x.
- **24.2 (protocollo DISK v2, stessi tag)**: frame con count (≤7/IPC, bound
  del ring), 1 comando PIO per run (`AtaDisk::read/write_sectors`), 1
  FLUSH CACHE per write (prima: comando+flush a settore). Da sola sopra
  24.1: overwrite 4K 36 ms → 30 ms. `BlockSource::{read,write}_sectors`
  (default a loop, `IpcDisk` in vero multi); DEV relay intatto.

## Lezione 24.2 — heap dei server: niente `Vec` temporanei per-op

A metà 24.2 i bench crollavano progressivamente (write ramfs 25 µs →
2 ms) in proporzione alle op FAT precedenti — con gate verde e risultati
corretti. Diagnosi (strumento temporaneo `civis::heap::heap_stats`,
mantenuto): la free-list first-fit di cardo cresceva di ~1 blocco a op
FAT (temp `Vec` 4K/512 B liberati tra blocchi vivi, mai coalescibili) e
ogni allocazione paga O(n) + O(n²) di `coalesce()` su tutte le op
successive, anche ramfs. Cura: hot path FAT zero-alloc — parse dir
incrementale con offset aritmetici, run in buffer stack a chunk ≤8,
risposta read in stack (count ≤ 4096 già garantito). Regola: **nei server,
mai allocazioni heap nel percorso per-op** (solo a setup/mount).

## Lettura

- Lo stack FS+IPC senza disco vola (14–25 µs): **il collo di bottiglia è
  il percorso disco**, non l'IPC in sé (floor ~2 µs/op).
- Un settore da disco costa ~1.2 ms: 2 round-trip IPC + handoff scheduler +
  ~266 uscite KVM (polling PIO porta per porta).
- `fat_small_orc` (~21 ms per 25 B) e `fat_4K_oow` (~64 ms) mostrano il
  moltiplicatore: ogni op logica = MANY settori (walk catena FAT con re-read
  del settore FAT a ogni cluster, find per open, read-modify-write +
  FLUSH CACHE dedicato per settore in scrittura).
- Le latenze a singola op interagiscono col quanto scheduler (20 ms):
  il riferimento per le ottimizzazioni (24) è il throughput, non la
  latenza minima.

## Soglia di non-regressione

Peggioramento > 10% su una qualunque riga (stesso host KVM, media 3 run)
= fail. Rivalutare la baseline solo a parità di hardware e versione QEMU.

## 25 — cache settoriale write-through in block (ADR-0018)

Un solo strato di cache a blocchi nel driver (`userland/disk/src/cache.rs`:
256 entry, chiave fisica `(disco, lba)`, CLOCK, write-through, zero heap nel
per-op); `fat_memo` (24.1) rimosso da `cardo`. Protocollo `DISK_*` invariato.

Confronto A/B **sullo stesso host** (KVM, media 3 run, TSC ~1.6 GHz;
la tabella 24 sopra e' di un altro host e NON e' confrontabile):

| Op | 24 stesso host (cyc/op) | 25 (cyc/op) | Effetto |
|----|-------------------------|-------------|---------|
| `zero_1B` | ~5.9 K | ~6.9 K | invariato (no disco) |
| `sda_512B_seq` | ~3.04 M | ~3.75 M | invariato entro il rumore¹ |
| `fat_small_orc` | ~6.14 M (~6 KiB/s) | ~49 K (~838 KiB/s) | **~126x** |
| `ramfs_4K_write` | ~93 K | ~117 K | invariato entro il rumore¹ |
| `ramfs_4K_read` | ~64 K | ~76 K | invariato entro il rumore¹ |
| `fat_4K_oow` | ~86 M (~81 KiB/s) | ~46 M (~146 KiB/s) | **~1.9x** |

¹ Rumore misurato: su questo host run identici dello stesso binario variano
±20–40% sulle op brevi (DVFS: tsc 1.62–1.68 GHz tra run; `fat_4K_oow`
baseline 57–97 KiB/s). Si dichiara solo cio' che supera di molto il rumore:
le re-read degli stessi settori (small FAT) non pagano piu' PIO; gli
overwrite pagano ancora PIO+FLUSH per settore (write-through) ma le re-read
di FAT/dir vanno in cache; il sequenziale freddo resta PIO (niente
read-ahead in 25, volutamente).

Hit rate: bench 74% (1514 hit / 534 settori via PIO), suite 91%.
Gate invariato (5/5 + 7/7 + 44/44 + shell 30/30).

## 38 — ATA DMA + IRQ (ADR-0029)

Motore Bus-Master PIIX in `block` (staging 1 pagina via `SYS_DMA_ALLOC`,
PRD split 64K, `READ/WRITE DMA EXT` UDMA2, fallback PIO per-op; protocollo
`DISK_*` invariato, DEV relay resta PIO) + attesa event-driven del
completamento (IRQ14/15 → notify, wakeup-preemption centrale in `notify_irq`,
guardie reply in `pop_msg` per notify/EXIT).

Confronto A/B **sullo stesso host** (KVM, media 3 run, TSC ~4.42 GHz; le
tabelle 24/25 sopra sono di altri host e NON confrontabili — poll = tree
38.1c, event = 38.2 con preemption):

| Op | poll 38.1c (cyc/op) | event 38.2 (cyc/op) | Effetto |
|----|---------------------|---------------------|---------|
| `zero_1B` | ~8.7 K | ~8.8 K | parità (no disco) |
| `sda_512B_seq` | ~5.47 M | ~5.55 M | parità (DEV relay = PIO in entrambi) |
| `fat_small_orc` | ~1.25 M | ~1.22 M | parità entro il rumore |
| `ramfs_4K_write` | ~112 K | ~116 K | parità (no disco) |
| `ramfs_4K_read` | ~61 K | ~61 K | parità (no disco) |
| `fat_4K_oow` | ~18.9 M | ~18.3 M | parità entro il rumore |

Tutte le righe entro la banda ±10% (soglia repo): nessuna regressione,
nessun miracolo — le op sono device-bound e il guadagno è altrove:

- **CPU non più bruciata in poll** (per costruzione): il poll 38.1c spinnava
  ~device-time a transfer a priorità Normal; l'event-driven dorme in `recv`
  e si sveglia via IRQ con switch diretto. Misura diretta (`ticks_used` di
  block ogni 512 xfers): su questo host (IO cached, device ~50 µs) le due
  versioni sono indistinguibili (+17–19 tick/512 in entrambi — il costo
  dominante resta memcpy/handling/ring, identico); il risparmio scala col
  tempo-device e conta sotto carico o su device lenti. Dichiarato il bounds,
  non gonfiato il numero.
- **Latenza IRQ→processo sub-tick per tutti i driver** (tasti inclusi) +
  prerequisito per audio CBS e server-run async (parcheggiati).

Cosa NON ha funzionato (tenuto a lezione, come 24.2-heap):

- Event-driven senza preemption: ogni wait pagava ~1 tick di wake differito
  (firme: `cyc_op` identici tra run = multipli di tick) — fat_small
  1.3M→180M cyc (~140x), fat_4K_oow 20M→810M (~40x). L'IPC sync fa handoff
  diretto, l'IRQ era l'unico wakeup differito: la preemption lo chiude.
- EXIT altrui in `wait_dma` clobberava la reply (canale reale) → wedge
  permanente a fine suite (morte usertests durante shell-load), visto in
  `test-shell.py`. Fix alla radice (`pop_msg` salta anche gli EXIT: rispondere
  a un morto è impossibile per disegno) — da userland la reply non si può
  ri-armare (niente `reply_to`).

Gate invariato (5/5 + 7/7 + 52/52 + shell verde); `ev_wait`≈transfer,
`fb=0`, `abort=0` su 4000+ transfer; `irq_drained` conta i re-fire
level-triggered (deterministici: identici tra run).

## 53 — Misura bulk P4 (round-trip vs dimensione)

Solo misura (Fase 53, P1–P5 OS-first verso ArcaFS): sweep 4K/16K/64K su
ramfs + FAT caldo/freddo, zero cambi di formato, zero pagine extra. Righe
`bulk_*` in `testland/bench` (stesso harness/`bench.sh`, mai nel gate).

Metodologia (lezioni apprese incluse):

- Freddo = **file distinti + spoiler unico**: ogni iter tocca settori mai
  visti (dati + catene FAT freddi; la dir padre va hot dopo iter 1, caveat
  documentato). Il primo tentativo (spoiler prima di OGNI iter) misurava
  spoiler+op con varianza dello spoiler (±25%) superiore al segnale —
  scartato dopo una campagna che lo ha provato: i cold venivano flat ~1.1G
  e persino inferiori agli hot. I numeri sotto sono op puri.
- `bulk_spoil_300sec` resta come riferimento metodologico (300 settori ≈
  costo eviction, ~1.1G cyc su questo host).
- Metodologia uniforme open/op/close per iter a offset 0 (steady state);
  b4/b5 restano gli anchor storici (grow-walk / oow). ramfs: serie singola
  (hot=cold in RAM, niente DISK/cache). Iter decrescenti con la size.
- Audit CAP single-source: `RING_DATA_CAP=4088` + `RING_MAX_PAYLOAD=4000`
  restano l'unica sorgente in `civis` (unico straggler trovato e fissato:
  `porta` clippava a letterale `4000`). Bound distinti intoccati: server
  `expect` 4096 (scratch per-op), `DISK_MAX_SECTORS` 7×512=3584 (fit ring),
  DEV relay 4096 (pre-esistente).

Campagna sullo stesso host (KVM, media 3 run, TSC ~1.66 GHz; tabelle
precedenti di altri host NON confrontabili; spread = max−min sui 3 run):

| Op | cyc/op medio | KiB/s medi | Spread | Lettura |
|----|--------------|------------|--------|---------|
| `bulk_ramfs_16K_write` | ~340 K | ~79 K | 10% | lineare da b4 |
| `bulk_ramfs_16K_read` | ~348 K | ~78 K | 15% | come write |
| `bulk_ramfs_64K_write` | ~1.30 M | ~83 K | 16% | ~20 cyc/B costanti |
| `bulk_ramfs_64K_read` | ~1.29 M | ~84 K | 11% | zero-copy al floor memcpy |
| `bulk_fat_4K_write_hot` | ~19.2 M | ~355 | 29% | ~4.7K cyc/B |
| `bulk_fat_4K_read_hot` | ~2.05 M | ~3.3 K | 45% | metadata ~2M fissi |
| `bulk_fat_16K_write_hot` | ~63.7 M | ~381 | 37% | ~3.9K cyc/B |
| `bulk_fat_16K_read_hot` | ~12.8 M | ~2.2 K | 62% | costo/B che cresce |
| `bulk_fat_64K_write_hot` | ~275 M | ~419 | 63% | ~4.2K cyc/B, lineare |
| `bulk_fat_64K_read_hot` | ~83 M | ~1.3 K | 18% | superlineare (v. sotto) |
| `bulk_fat_4K_write_cold` | ~20.8 M | ~328 | 31% | ≈ hot (write-through) |
| `bulk_fat_4K_read_cold` | ~3.76 M | ~2.1 K | 95% | 1.8x hot (metadata) |
| `bulk_fat_16K_write_cold` | ~75 M | ~365 | 30% | ≈ hot |
| `bulk_fat_16K_read_cold` | ~11.9 M | ~2.4 K | 54% | ≈ hot entro rumore |
| `bulk_fat_64K_write_cold` | ~253 M | ~425 | 11% | ≈ hot |
| `bulk_fat_64K_read_cold` | ~67.8 M | ~1.6 K | 8% | ≈ hot entro rumore |

Lettura per A2 (decisione rinviata ai numeri — eccoli):

- **Costo ~lineare nei chunk**: FS a 4000 B/chunk + DISK a 7 settori/run
  dominano; frame più grandi = meno round-trip (write ~4.2K cyc/B costanti,
  read con quota fissa metadata ~2M cyc ≈ 1.3 ms per op).
- **Read superlineare** (501 → 783 → 1269 cyc/B da 4K a 64K): walk di
  catena + run per cluster, un IPC DISK per settore — il fan-out
  (`R_OBJ_MGET`: un IPC, N blob) attacca esattamente questo.
- **Cold ≈ hot sul bulk dati** (write identiche per write-through; read
  grandi entro rumore): la cache salva i metadati, non i dati — A2 non può
  contarci per il bulk.
- ramfs al floor (~20 cyc/B): lo zero-copy c'è già; il multi-frame serve
  al FAT, non alla RAM.

Gate invariato (5/5 + 7/7 + 57/57 + shell verde); bench mai nel gate.
