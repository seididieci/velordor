# ADR-0040: Pantheon dei nomi + rename a fasi (R0–R8)

**Status**: Accepted (R0 — solo docs; il codice segue nelle fasi R1–R9).
Emendamento R0b: entrano `Vestigia` (logging) e `Porta` (terminale).

## Context

Velordor e' cresciuto con nomi di comodo (`userdisk`, `sched_rt`,
`channels`) che non raccontano piu' l'architettura: un microkernel con
isolamento userspace, IPC per nome, tempo garantito e filesystem nativo.
Serve un vocabolario stabile — metafora per orientarsi, nomi veri nel
codice — senza flag day: il rename avviene **un pezzo alla volta**, una
fase un componente, sempre a gate verde.

## Decision

### Il pantheon

Velordor e' il sistema operativo. Dentro:

| Nome | Ruolo | Metafora |
|------|-------|----------|
| Velord | il kernel | il sistema |
| Vela | ciò che permette al kernel di "navigare" l'hardware (driver + hub `/dev`) | la vela |
| Arca | ciò che conserva i dati (filesystem: formato, volumi, object store) | l'arca |
| Relay | ciò che permette ai componenti di comunicare (IPC per nome + canali) | — (nome tecnico) |
| Ordo | ciò che ordina l'esecuzione (scheduler RT) | l'ordine |
| Aegis | isolamento temporale / bandwidth reservation (CBS), dentro Ordo | lo scudo |
| Arc | memoria (fisica + virtuale + heap) | — (nome tecnico) |
| Cardo | il perno del traffico file/driver (server FS: mount, registry, smistamento) | la cerniera |
| Vestigia | le tracce che il sistema lascia (logging L1) | le vestigia |
| Porta | la soglia d'ingresso dell'utente (terminale, line discipline) | la porta |

Fuori schema (nomi invariati, per sempre o fino a nuova ADR): `shell`,
`uptime`, `posix`, `time`, helper di test (`usertest*`, `utcbstest`,
`hogheap`, ...). I device path (`/dev/input/*`, `/dev/kbd/*`, ...) non
cambiano mai: namespace device ≠ nomi servizi.

### Mapping vecchio → nuovo

| Vecchio | Nuovo | Fase |
|---------|-------|------|
| kernel (`velordor-kernel`) | `velord` (package) | R1 |
| `sched_rt` (+dir), `process`, `context` | `ordo::sched` (+ `process`, `context`) | R1 |
| `cbs.rs` | `ordo::aegis` (CBS dentro lo scheduler, dov'e') | R1 |
| `channels.rs` | `relay::channels` (`syscall/` resta: entry point) | R1 |
| `phys_mem`, `vmm_user` (+dir), `vmm`, `heap` | `arc::{phys,virt,heap}` (+ `vmm` dove sta) | R1 |
| (nuovo) `libs/vela` | casa del codice driver condiviso (`hub\|block\|input\|gpu`) | R2 |
| `userkbd` | `kbd` (dir `userland/kbd`, `Service::Kbd` invariata) | R3 |
| `userconsole` | `gpu` (display/dest, `Service::Gpu`) | R4 |
| `userdevfs` | `vela` hub (display, `/dev` invariato, `Service::Vela`) | R5 |
| `userdisk` | `block` (embedded: tabella kernel + path init, `Service::Block`) | R6 |
| `userfs` | `cardo` (embedded + piu' citata, `Service::Cardo`) | R7 |
| `userlog` (+ mod `libr::log`) | `vestigia` (embedded, `Service::Vestigia`; `LOG_*` invariati) | R8 |
| `usertty` | `porta` (`Service::Porta`; device path intoccati) | R9 |

Note: `tty` non si tocca; i dest FAT restano `X.bin` corti e stabili dove
sono (si rinominano solo con la fase del driver, in coppia con init:
`tty.bin`→`porta.bin`, `console.bin`→`gpu.bin`, `vestigia.bin` e' 8.3
valido); manifest/policy si rigenerano da soli (glob `*.bin`) — verificare,
non editare. I tag wire restano stabili anche quando il servizio cambia
nome (precedente: `TIME_NOW` per `Time`, `LOG_*` per `Vestigia`). Repo, OS
e titoli restano `Velordor`.

### Regole standing per fase (non negoziabili)

1. Una fase = un componente, un commit, gate verde (`5/5 + 7/7 + 40/40 +
   58/58`, zero FAIL/PANIC/FAULT) + voce di cronologia.
2. Checklist meccanica: `build_one` + inject dest + `NAMED_BINARIES` (se
   embedded) + `SvcMeta`/`expected_*` + shell map + test refs +
   `rg oldname == 0` in code+scripts+tests (`-w` obbligatorio per `porta`:
   sottostringa di `riporta/trasportato`; docs ammessi fino a R-final,
   con questa tabella come riferimento).
3. Mai due fasi in un commit; mai rename + comportamento insieme.
4. Le varianti `Service::*` si rinominano con la fase driver (match
   esaustivi: il compilatore trova tutti i siti — kernel, init, shell,
   test). I discriminant (ABI) non si toccano mai.

## Consequences

### Positive

- Vocabolario stabile prima del codice (questa ADR e' il riferimento
  durante le fasi, non la memoria di nessuno).
- Ogni fase e' reversibile da sola (un commit) e verificabile (gate).
- Il compilatore fa da rete: variant e `mod` esaustivi, niente rename
  silenziosi a meta'.

### Negative

- R1 tocca molti path `crate::` nel kernel in un colpo solo (meccanico
  ma diffuso — mitigato dalla regola 3: zero comportamento insieme).
- I docs restano misti fino a R-final (vecchi nomi fuori dal codice):
  dichiarato e tracciato, non marcio per caso.

### Neutral

- `Relay`/`Arc` restano nomi tecnici senza metafora: voluto (meccanismi,
  non personaggi).
- `Cardo` e' l'unico nome nuovo fuori dallo schema iniziale: giustificato
  dal ruolo (perno, non filesystem — quello e' Arca).

## Alternatives Considered

- **Flag day totale**: scartato — superficie di regressione (stringhe
  load-bearing: `NAMED_BINARIES`, `SpawnMeta`, `ps`, 8.3, policy, 34 file
  docs) contro guadagno funzionale zero.
- **Solo docs, mai codice**: scartato — la metafora senza codice marcisce
  (due vocabolari per sempre).
- **userfs → `arca`**: scartato — confonde server e filesystem (il
  multiplexer VFS non e' ArcaFS); `vfs` scartato perche' fuori metafora,
  `non rinominare` scartato perche' lascia il pezzo piu' citato fuori
  dallo schema.
- **Aegis modulo fratello di Ordo**: scartato — il CBS vive dentro lo
  scheduler (separarlo e' artificiale); sta in `ordo::aegis`.
