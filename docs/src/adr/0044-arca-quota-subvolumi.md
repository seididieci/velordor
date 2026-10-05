# ADR-0044: Quota e subvolumi ArcaFS (A3)

## Status

Accepted

## Context

ArcaFS (56.x, ADR-0042/0043) non ha budget: qualunque bucket cresce finche'
il volume ha blocchi liberi e l'unico rifiuto e' l'allocatore secco (`ERR`
opaco). La spec (`arcafs.md` §7) chiede subvolumi con budget
(`quota_blocks`, `used` senza doppio conteggio), enforcement al put
(`ERR_NOSPACE` prima di allocare, mai mezze scritte) e snapshot contro il
budget di chi li trattiene. I ganci ci sono gia': `R_STATVFS` dichiara la
quota futura, il superblock persiste `alloc_hint`, la foglia meta persiste
la tabella snapshot (`meta_store`/`meta_load`).

## Decision

Subvolume = bucket (arcafs.md: un bucket = un subvolume = un mount; niente
nuovi oggetti di primo livello). Per bucket, due numeri:

- `quota_blocks: u64` — budget blocchi dati, 0/assente = illimitato (mai
  bootstrap che rompe il boot: il seed non e' mai gated).
- `used_blocks: u64` — blocchi dati DISTINTI trattenuti dal bucket, ricalcolo
  esatto a ogni check (walk secondary del bucket + catene di TUTTE le
  versioni incluse le pinnate + pinnati delle snapshot ANCHE a nome
  cancellato: lo snapshot conta contro chi lo trattiene, mai buco da
  delete-con-snapshot-vivo). Niente contatori incrementali (niente drift,
  niente bug di trim).

Unita' = blocchi dati overflow (catene `OV_CHUNK` da 3552 B). Fuori quota
per disegno (dichiarato, non dimenticato): nodi indice (condivisi tra
bucket, attribuzione arbitraria), valori inline ≤512 B, foglia meta. La
quota e' un budget dati, non un conteggio blocchi fisici totale — a scala
gate i nodi sono decine, i dati migliaia.

Enforcement PRIMA di allocare, su ogni path che scrive dati nuovi nel
bucket: `disk_put`/`ns_put` (need = blocchi del blob risultante + slack
COW), `disk_snap_rollback` (stesso bucket, nuova versione), `disk_snap_clone`
(bucket dst, somma dei clonati). Oltre budget → `ERR_NOSPC` (`!0-11`,
`Error::NoSpace`, ENOSPC al bordo). Stima conservativa: il check e' pre-put,
il trim post-put puo' solo liberare — si rifiuta presto, mai troppo tardi.

Persistenza: la tabella quota vive nella foglia meta (`quota/<bucket>`,
stesso blocco degli `snap/`, scritta da `meta_store` su `quota_set` e sulle
op snapshot come oggi). `used` e' solo RAM (ricostruito dallo scrub al bind:
la misura ESATTA e' lo scrub, non servono due fonti che divergono).

Solo motore disco (A3): il backend in-RAM resta senza quota (path legacy di
bootstrap/test, mai dati di produzione).

## Consequences

- Nuova sentinella `ERR_NOSPC` + `Error::NoSpace` (meccanismo, non POSIX:
  `to_errno` la mappa a ENOSPC al bordo come le altre).
- Sub-op debug `QUOTA_SET`/`QUOTA_GET` su `R_ARCA_DEBUG` (scaffold come gli
  altri: A7 li gatta in un punto solo).
- Test 51-54 in testsarca (over-budget tipizzato, indipendenza bucket,
  persistenza quota, coerenza `used` con snapshot/delete); unit host su
  `MemStore` per il round-trip meta.
- A4 (ABAC) trova i budget su cui appoggiare le policy; A5 rivaluta lo
  slack di placement con numeri reali.
