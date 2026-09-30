# ADR-0041: Personalità POSIX separata — meccanismo `civis` + `flavours/posix` (Fase 58)

## Status

Accepted (Fase 58.1 — solo docs; il codice segue nelle fasi 58.2–58.6).
Estende [ADR-0025](0025-native-model-personalities.md) (POSIX = personalità) e
[ADR-0040](0040-naming-pantheon.md) (pantheon dei nomi), senza riscriverli.

## Context

ADR-0015/0025 hanno stabilito che POSIX è **API/personalità**, non ABI del
sistema: il kernel resta neutro e la traduzione vive al bordo. In pratica però
il crate `libs/libr` — unico linkato da *tutti* i binari userland e test —
mescola tre strati (dichiarati in `libr/src/lib.rs`): meccanismo neutro
(`sys`/`ipc`/`heap`/`task`/…), personalità POSIX (`posix` = errno, `stdio` =
vfd+redirect) e moduli misti POSIX-named ma di meccanismo (`fs`, `spawn`,
`print`, `args`). La separazione esiste **sulla carta**, non nelle dipendenze:
la personalità non è estraibile né sostituibile, e un crate nativo si porta
dietro il vocabolario POSIX.

Il rename a fasi (ADR-0040) ha dato un pantheon ai componenti, ma la libreria
di sistema condivisa è rimasta fuori schema. Serve un nome per il *meccanismo*
e una casa esplicita per la *personalità*, così che una seconda personalità
riusi il meccanismo invariato (prova del punto 4 di ADR-0025).

## Decision

1. **Meccanismo = `civis`.** `libs/libr` (package `libr`) → `libs/civis`
   (package `civis`; *civis* = cittadino: il meccanismo comune a ogni
   personalità). Contiene: `sys`, `ipc`, `heap`, `scratch`, `task`, `pio`,
   `pci`, `tsc`, `test`, `error`, `time`, `vestigia`, `fs/*`, `print`, `args`,
   e la parte meccanismo di `spawn` (`spawn`/`spawn_image`/`exec_image`/
   `exec_image_args`/`service_*`/`peer_*`/`map_physical`).
2. **Personalità = `flavours/posix/libr`** (package `libr`, nome invariato).
   Contiene: `posix` (`to_errno` + `E*`), `stdio` (tabelle vfd, redirect,
   `RedirEntry`, `stdio_restore`), `fork`, `exec`/`exec_env` path-based e
   `serialize_argv*`. Dipende da `civis`; i binari POSIX linkano entrambi.
3. **Confine (non negoziabile).** `args` (convenzione argv/env, dato neutro
   ADR-0025 §Neutral), i wrapper fd (`open`/`read_fs`/`write_fs`/`close`/
   `readdir`/`pipe`/`dup_*`/`lseek`/`mount`/`stat`/…, API client verso cardo) e
   `exec_image` restano in `civis`: li usano anche i server nativi. Va in
   `flavours/posix` **solo** la traduzione di personalità.
4. **Inversione delle dipendenze.** `civis` non riferisce mai POSIX. Oggi
   esistono tre riferimenti meccanismo→POSIX, risolti così:
   - `print::flush` instrada via hook `civis::persona::route_out: Option<fn(&[u8]) -> bool>`,
     installato dall'entry POSIX; senza hook → seriale (nativo invariato).
   - `REDIR_MAGIC` diventa prefisso argv **riservato neutro** in `civis::args`
     (il layout argv è già convenzione di dati ADR-0025); la semantica
     (parse/claim/`set_stdio`) resta in `libr::stdio`.
   - due macro `entry!`: `civis::entry!` (shim nudo, nativo) e `libr::entry!`
     (chiama `stdio_restore` + installa l'hook di routing, POSIX).
5. **Struttura.** `flavours/posix/{libr, server, shell, cli, tests}`;
   `flavours/posix/server` = posix-server (`userposix`), `shell` = shell
   (`usershell`), `cli` = programmi lanciabili POSIX (`runhello`). Restano
   nativi e fuori flavour: `userland/` (init, cardo, block, gpu, kbd, vela,
   porta, vestigia, time, uptime). **Analisi `arca`**: non usa alcuna API POSIX
   (solo meccanismo: `open`/`read_fs`/`disk_*`/`args`), quindi casa nativa
   `userland/tools/arca`; usa pero' `libr::entry!` (non `civis::entry!`) perche'
   e' un programma lanciabile dalla shell e deve onorare il redirect POSIX
   (comportamento identico a prima: il redirect si ripristina solo nel crate
   che linka la personalita'). Gli output `.bin` del flavour vivono in
   `flavours/posix/build` (separazione esplicita dei path).
6. **Fase 58** (58.1–58.6), una per commit, gate verde in ognuna:
   - **58.1** docs/ADR (questa).
   - **58.2** rename `libr`→`civis` (meccanismo), diff meccanico compiler-guided.
   - **58.3** estrazione crate `flavours/posix/libr` + inversione punto 4.
   - **58.4** spostamento package userspace POSIX (`server`/`shell`/`cli`).
   - **58.5** estrazione suite test POSIX (`flavours/posix/tests`): il gate
     guadagna una riga, `usertests` scende di conseguenza.
   - **58.6** chiusura docs (percorsi 00/06/09/11/12/14, AGENTS, run-tests).
   La fase si inserisce nella sequenza principale; le fasi ArcaFS A3–A8 già
   etichettate `58+` slittano a `59+` (rinumerazione del pregresso, §Alternative).

## Consequences

### Positive

- La personalità è estraibile e sostituibile: `civis` è neutro per costruzione
  (regola verificabile con `rg "posix::" libs/civis/src` → zero).
- I binari nativi smettono di dipendere dal vocabolario POSIX; il confine
  meccanismo/personalità diventa una dipendenza Cargo, non una convenzione.
- Prova concreta del punto 4 di ADR-0025: una seconda personalità (nativa)
  riuserebbe `civis` senza toccarlo.

### Negative

- Diff ampio e trasversale (rename su ~90 file + riorganizzazione build);
  mitigato dalle fasi a gate verde e dal rename compiler-guided.
- `58.5` cambia il conteggio del gate (nuova riga `[posixtests]`): va allineato
  in `11-testing.md`, AGENTS e `run-tests.sh` nello stesso commit.
- Doppio nome da tenere a mente (`civis` meccanismo, `libr` personalità): la
  scelta è voluta (il nome `libr` resta il punto d'ingresso dei programmi).

### Neutral

- `arca` e `uptime`, pur spostati/sondati, sono nativi: non entrano in
  `flavours/posix` (analisi in `Context`/`Decision` §5).
- I tag wire, i nomi `.bin` e i `Service::*` non cambiano: nessun impatto ABI.
- La fase è di sola struttura: **zero cambi di comportamento**.

## Alternatives Considered

- **Lasciare `libr` misto**: scartato — la personalità resta non estraibile e
  il confine si erode a ogni primitiva (il rischio che ADR-0025 vuole evitare).
- **Facade `libr` che riesporta `civis`**: scartato — nasconde la dipendenza
  invece di dichiararla; import espliciti (`civis::`/`libr::`) rendono visibile
  quale strato si usa.
- **Spostare anche i wrapper fd in POSIX**: scartato — `open`/`read_fs` sono
  l'API client di cardo usata dai server nativi; li trascinerebbe sulla
  personalità, invertendo il verso.
- **Chiamare il meccanismo `cardo`**: scartato — `cardo` è già il server FS
  (`Service::Cardo`, ADR-0040): collisione di package e di prosa.
- **Continuare la serie R di ADR-0040 (R10–R15)**: scartata — la serie R era il
  cantiere di rename chiuso con R-final; il lavoro merita una fase propria nella
  sequenza principale (58), accettando di rinumerare le fasi ArcaFS `58+`→`59+`.
- **Numerare i test POSIX spostandoli ma senza nuovo binario**: scartato — la
  personalità merita una suite propria; il gate esistente resta per il resto.

## References

- [ADR-0015](0015-posix-api-libr-protocollo-interno.md) (POSIX API di `libr`),
  [ADR-0025](0025-native-model-personalities.md) (personalità),
  [ADR-0040](0040-naming-pantheon.md) (pantheon + fasi R di refactor)
- `libs/libr/src/lib.rs` (stratificazione meccanismo/personalità),
  `docs/src/14-cronologia-fasi.md` (voci 58.1–58.6)
