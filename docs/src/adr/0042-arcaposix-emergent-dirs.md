# ADR-0042: Namespace POSIX su ArcaFS — directory emergenti + set transient in RAM (56.3)

## Status

Accepted (Fase 56.3 — implementato in `userland/cardo`, suite 50/50).

## Context

ArcaFS esponeva solo l'object-store nativo (`R_OBJ_*`, Fase 55/56.2): la vista
POSIX (`userland/cardo/src/arca.rs`) era uno stub che rifiutava tutto e il
boot viveva su FAT+ramfs. `arcafs.md` §5 prevedeva directory *persistenti*
(marker reali su disco, rinviati a 56.3). Per diventare filesystem di default,
ArcaFS deve servire path gerarchici — ma i marker persistenti costano un
formato, un libro mastro e la sua GC.

## Decision

1. **Directory emergenti, mai marker su disco.** Una dir esiste ⟺ ha chiavi
   col prefisso (o è root). I file si scrivono con path completo nel bucket
   `ns` (separato da `sys`/dati); i parent emergono da soli. Niente formato
   nuovo, niente libro mastro: la persistenza è strutturale (dopo
   kill+remount le piene riemergono dalle chiavi).
2. **`mkdir` non scrive mai** (niente commit): inserisce path + antenati in
   un set transient in RAM nell'istanza `ArcaFs` (perso a restart/remount —
   le vuote spariscono, le piene riemergono). Idempotente; file in `rel` =
   `EXISTS`, antenato file = `NOTDIR` (come ramfs).
3. **`rmdir` mai silenziosa.** Dir con figli = `NOTFOUND` (come ramfs);
   dir RAM vuota = tolta dal set; mai-esistita = `NOTFOUND`; root e file =
   errore. `remove(file)` non tocca il set (la dir resta visibile finché non
   la si rimuove).
4. **Un solo motore per volume.** La vista POSIX usa il `DiskEngine` globale
   via `ArcaWith` (legame mount+motore solo a uuid combaciante, verificato
   in `mount::arca_with`); altri volumi = errore loud, mai dati altrui.
   Niente `Rc`/`RefCell`, niente doppi handle sullo stesso volume (la
   divergenza freelist corromperebbe — vedi `btree_drv`, un solo
   proprietario). Senza motore si cade sullo stub di prima (init ripiega su
   FAT come sempre).
5. **Scansione per prefisso** (`BTree::scan_prefix`, full-scan O(n) con
   guardia anti-loop): per un FS di boot va bene; la range-scan con discesa
   resta futura. `mtime` dir = max(mtime figli, mtime RAM); `statvfs` onesto
   (blocchi 3584, liberi illimitati come ramfs — sensore vero con la quota).
6. **`R_SYNC GROUP` = commit esplicito** (no-op logico col commit per-op,
   loud a IO fallito); il resto invariato.

## Consequences

- `arcafs.md` §5 riscritto (marker → set transient, perdita delle vuote
  dichiarata come semantica). `put_chunk(0)` = fresco: gli overwrite
  parziali passano da read-modify-write nel provider.
- Estende [ADR-0025](0025-native-model-personalities.md) (presentazione
  POSIX al bordo, core nativo) senza toccare il confine `civis`.
- Prossimo: root su ArcaFS + seed dal volume (Fase 2 del piano), poi cutover
  boot e FAT secondaria.
