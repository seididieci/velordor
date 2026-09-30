# ArcaFS — specifica (bozza A0)

Filesystem nativo non-POSIX per Velordor: object store versionato con COW,
snapshot, quota, ACL/ABAC. POSIX solo come vista (mapping sintetico).
Filosofia ADR-0025: nativo dentro (userfs), personalita' al bordo (libr);
provider trait ADR-0038; policy/identita'/sandbox ADR-0037.

Stato: sessione guidata A0 completata (decisioni T0–T10) + piano OS-first
P1–P5 chiuso (Fasi 50–54) + A1+N0 chiuso (Fase 55: object store, mount
MBR/GPT, `sys` seedato, init dual-mode) + A2/56 CHIUSA (56.1 versioni in RAM;
56.2a formato+allocatore; 56.2b B+tree COW + commit su disco; 56.2c
recovery/orphan-GC + snapshot persistenti + sys-dal-volume) + 57/Logging-L1
CHIUSA (userlog RAM-first, bucket per identita', ADR-0039; L0 cancellato).
Prossimo: 58+ (A3 quota/subvolumi, A4 ABAC, ...). Packing S1/S2,
`R_OBJ_MGET` e marker dir persistenti (56.3) restano rinviati.

> Nota sui gate: i numeri citati altrove sono snapshot storici; il gate
> corrente vive in `docs/src/11-testing.md` e in `ROADMAP.md`
> (attuale: `[testfs] 5/5` + `[testfat] 7/7` + `[usertests] 57/57`).

## 0. Vision e principi

- Non-POSIX nel profondo (`open/read/write` non sono la fondazione),
  integrato con l'OS: `libr` e binari di sistema parlano nativo ArcaFS.
- Vincoli software mai sacri (ring, `DISK_*`, single-thread riscrivibili
  quando un topic lo richiede); vincoli hardware rispettati (settore 512B,
  seek HDD, RAM finita per le cache).
- Ogni struttura a cardinalita' futura ignota: **inline + overflow**
  (mai tabelle a dimensione fissa che diventano muri).
- Dimensioni negoziate nel superblock, mai costanti cablate (`block_size`,
  soglie S1/S2, cap transient set).
- `no_std` e' **temporaneo**: core `no_std`-first con feature `std`
  opzionale, host tooling (`arca`) in std, verso una std minimale per il
  self-hosting.
- **Integrazione col provider** (ADR-0038): ArcaFS e' un
  `LocalFs`/`LocalFsDyn` in `userfs`; l'API nativa `R_OBJ_*` e' additiva
  (dettagli §4).

### Posizionamento: cosa ArcaFS non vuole essere (anti-ZFS)

- **Non un "ZFS/btrfs migliore".** Sul meccanismo di storage (COW, snapshot,
  checksum, quota, RAID, TXG, volumi) ArcaFS e' ZFS-shaped: la meccanica e'
  derivativa e **non e' il punto**. Rincorrere ZFS feature-per-feature
  (send/receive, compressione, dedup, RAID oltre A6) e' la trappola: si
  perde su terreno maturo con risorse hobbistiche.
- **Cosa lo distingue, e va difeso**:
  - object-native, non file-native: il fondamento e' `bucket`+`chiave`+
    versione, POSIX e' una proiezione;
  - identita' UUID con versioni esplicite (MVCC da database, non inode);
  - capability/ABAC con soggetto = identita' misurata dell'app, non ACL
    aggiunte a posteriori;
  - motori (DB, VM/block, vector) **fuori** dal FS: il FS da' primitive, non
    query;
  - il FS come **updater atomico dell'OS** (volume `sys` immutabile +
    overlay + rollback).
- **Criterio di unicita' (e anti-deriva):** una feature entra in spec solo se
  (a) serve a uno di questi assi, oppure (b) e' necessaria a POSIX/
  self-hosting. "Lo fa anche ZFS" non e' una ragione per farla; "lo fa ZFS
  meglio" e' una ragione per **non** farla.
- **Il test che conta:** finche' l'unico consumatore reale e' il toolchain
  POSIX, l'unicita' e' teorica. Il trigger di ADR-0025 (2+ app native senza
  POSIX-ismi) va preso sul serio: almeno una app nativa che parli `R_OBJ_*`
  va costruita prima di dire che il modello regge. Il primo consumatore e'
  `init` (servizi per `object_id`, §13); la prima app nativa e' la Vault di
  artefatti.

## 1. Modello dati (T1)

- Namespace piatto: `bucket` + `chiave` opaca. `/` solo convenzione di
  listing (pattern S3); la gerarchia Unix e' composizione di mount, non
  struttura del FS.
- Blob **versionati**: ogni put crea una versione, le vecchie restano per
  snapshot/GC. Put su chiave esistente = nuova versione (mai errore, mai
  sovrascrittura logica).
- Eccezione **bucket `block`** (tipo di bucket, §12): il **head e' unico e
  mutabile**, ogni put aggiorna il head senza creare versioni storiche; gli
  snapshot restano l'unica retention. Vale solo per i bucket dichiarati
  `block`; i bucket `object` (default) seguono la regola versionata.
- Identita': `object_id: u64` monotonico per volume, immutabile, mai riusato
  (disciplina F2: niente ABA). La chiave e' rinominabile, l'UUID no.
- Unicita' globale: `(volume_uuid, object_id)` — niente UUID-128, niente
  collisioni al merge (A8). Nota 56.2a: l'u64 resta (merge = A8, trigger
  per UUID-128 con superblock v2 o registry — il superblock ha 2 byte
  liberi, l'allargamento si paga solo quando serve); il `volume_uuid` e'
  auto-generato al `create` (OS RNG, `--uuid` solo override), mai default
  fisso.
- **Non content-addressed**: l'identita' e' `object_id`, non un hash del
  contenuto; niente dedup automatico in v1. Trade-off dichiarato: identita'
  e rename semplici e niente ABA, a costo di spazio duplicato. Un
  content-hash opzionale (`sys.content_hash`, §8) potra' abilitare dedup come
  ottimizzazione, mai come identita'.
- Chiavi relative (mount+bucket strippati); soglia inline ≈ 200B: i path
  realistici restano inline, l'overflow e' per chiavi patologiche.
- Bucket multipli come mount: `/`, `/home`, `/home/user` (annidato),
  `/var`, … — un bucket = un subvolume = un mount (quota/policy/snapshot
  seguono la granularita' del mount).

## 2. Indicizzazione (T2)

- **Primary B+tree per UUID** → `{versioni, quota, attributi}`: snapshot,
  clone, GC e quota parlano UUID, mai nomi (POSIX e' solo flavour). Nei
  bucket `block` (§12) il primary tiene `head + snapshot`, non una catena di
  versioni; secondary e stat denormalizzata restano invariati.
- **Secondary per `(bucket,key)`** → UUID + stat denormalizzata
  (size, mtime, version_head): listing in un range scan, `stat` senza
  toccare il primary.
- Nodi da **3584 B** (`block_size` negoziato, fisso v1) = 1 chunk `DISK_*`
  esatto; FNV-1a/64 + confronto byte esatto (mai solo-hash).
- Foglie con overflow per chiavi lunghe; discesa inalterata (separatori
  corti), +1 read solo alla conferma.
- Rename stesso volume = delete+insert solo sul secondary (atomico,
  zero copie); cross-subvolume = cp + tombstone (non atomico, dichiarato).
- **Perche' B+tree e non un Merkle tree**: il namespace e' piatto
  (`bucket`+`key`); snapshot/COW per-UUID non richiedono un albero di
  directory, e il path Unix e' composizione di mount, non struttura.
- Packing (implementazione A2, tipi riservati in A0): inline in foglia
  ≤ S1; pack sigillati ≤ S2 con mini-indice e **copy-out-on-write**
  (mai rewrite parziale); compattazione con la GC.

## 3. Superblock e layout (T3)

- LBA0 (+ shadow LBA1): `magic="ACFS"`, `version=1`, `block_size=3584`,
  `volume_uuid:u64`, `generation:u64`, `root_tree`, `refcount_root`,
  `alloc_hint`, `mountpoint[64]`, `auto:u8`, `flags` (dirty + feature),
  checksum FNV-1a self-verifying. Niente firma `55AA` (mai falsi mount FAT).
- Rilevabile con 1 read LBA0 + magic + bound (come `Fat32::mount`).
- Commit: shadow + flip; crash = generazione vecchia + orphan-GC
  (journal rivalutato solo se gli snapshot multipli lo impongono).
- Il volume possiede l'**intero device** di default (dischi dati: nessuna
  tabella partizioni). **Eccezione solo-boot (A1)**: sul disco di boot GPT
  (EFI) ArcaFS vive in partizione dedicata (type GUID proprio, da registrare;
  ESP intoccata) — superblocco e shadow sono **partition-relative** (LBA0+LBA1
  del nodo, mai assoluti: lo shadow a `base+1` non collide con l'header GPT a
  LBA fisico 1). Il loader EFI resta fuori scope (solo readiness data-plane:
  parser GPT in userdisk, `negotiate()` prova `ACFS` prima di `vfat`,
  `arca create --whole-disk`/`--in-partition`); `device_table` (§10) e' per il
  multi-device, non per partizioni.
- **Niente log append-only**: il commit e' shadow superblock + flip, il resto
  e' COW B+tree; recovery = generazione vecchia + orphan-GC.
- **Integrita' a due livelli**: (1) checksum veloce FNV-1a per lo scrub
  rapido (rileva corruzione accidentale, non un avversario — vedi threat
  model ADR-0026); (2) tag crittografico BLAKE2s (troncato a 128 bit nel
  footer, 256 bit nel seal del superblock) per tamper-evidence e
  `sys.content_hash` (§8). Libreria: standard da registry se supera i
  cancelli in-place (build freestanding `no_std` senza alloc, size entro il
  bound `SPAWN_IMAGE_MAX`, vettori RFC 7693, niente heap nel per-op),
  altrimenti reimplementazione propria (~500 righe, solo u32, auditabile);
  solo userspace (userfs + `arca` + test), mai kernel (ADR-0025). FNV-1a
  resta per `peer_info`/manifest/ABI u64 (hint + confronto esatto, mai
  sicurezza).
- Footer blocco: `(type, device_idx, gen, tag128)` — blocchi
  self-describing, scrub indipendente dal tree (dimensione esatta nel
  dettaglio A1).
- Pool uniforme con **hint di placement** (zone veloci, co-location:
  preferenze soft, mai vincoli); l'allocatore decide, A5 cambia politica.
- `device_table[8]` inline + `overflow_ptr` + `device_count`;
  `topology_gen` per i cambi (stale detection).
- Riservati: `net_cookie[16]` (A8), `vec_hook[8]` per oggetto (vector).

## 4. API nativa (T4)

- Dati: `R_OBJ_PUT/GET/DELETE/LIST` (+ chunk/commit espliciti oltre 4000B,
  indice chunk non offset); `GET_ID/STAT_ID` per UUID; `LIST` paginata con
  cursore opaco.
- Riservati: `R_SNAP_*` (A2), `R_ARCA_*` admin (A7).
- Errori `Result` tipizzati (`NOTFOUND/EXISTS/BUSY/NOSPACE`), mai errno.
- Pattern a 5 tocchi per ogni op: tag, expect, `op_bit`, wrapper libr, builtin.
- **Integrazione provider** (Fase 46–49, ADR-0038): ArcaFS si monta come
  `MountedFs::Local(Box<dyn LocalFsDyn>)`; `negotiate()` (`mount.rs`)
  riconosce `magic="ACFS"` a LBA0 e ritorna `fstype="arcafs"`; nuovo
  `AnyHandle::Arca(...)`. La vista POSIX passa da `LocalFs`; `R_OBJ_*` resta
  l'API nativa additiva.

## 5. Vista POSIX / mapping sintetico (T5)

- Oggetto = file, lista = readdir, dir **emergenti** (esistono ⟺ chiavi
  col prefisso; mai su disco).
- **Directory persistenti** (requisito self-hosting, **rinviato ad A2**):
  `mkdir` crea un marker reale, quindi la dir sopravvive a unmount/reboot;
  in A1 vale il transient set server-side (ottimizzazione in futuro, unica
  semantica in A1) con tag tipo riservato nel formato. Nessun cap
  che faccia fallire `mkdir -p` su molte directory (es. `tar x`, build).
  Trigger: porting toolchain.
- Lettura + append + delete + **write con offset ammesso** (nuova versione
  COW con la range patchata, costo dichiarato); `O_APPEND` resta il caso
  naturale. **`ftruncate` ammesso**: nuova versione con size ridotta e tail
  liberato.
- `stat`: size/kind/mtime dalla versione; `nlink` = versioni trattenute;
  `owner` = `creator_app` risolto (o hash corto); `group` = `-` (v1);
  `mode` = **proiezione ABAC valutata** (non memorizzata), calcolata dal
  **server** in base al chiamante — `LocalFs::stat` resta subject-agnostica
  (vedi §6); `chmod` = `ERR_READONLY` (si usa il tool di policy), ma non deve
  far fallire hard i build che lo invocano.
- `atime` non tracciato in v1.
- Da verificare durante il porting del toolchain (non vincolo ora):
  `symlink`/`link`, `utimes`, `chown`.
- I bucket `block` **non passano da questa vista**: sono volumi, esposti dal
  backend block device (§12), non file POSIX; `nlink`/versioni non si
  applicano.

## 6. Diritti e ABAC (T6/T8)

- Bit `OBJ_R/OBJ_W` separati + `SNAP` + `ADMIN`; fail-closed agli ignoti;
  subtree esteso a bucket/prefisso (gratis: l'op porta bucket nel payload).
- Motore in userfs (mai kernel): soggetti = `app_hash` oggi (+ `app_sign`
  e UID domani come attributi), subtree, ruoli-servizio; oggetti = xattr.
- Ereditarieta' bucket→chiave solo in restrizione; snapshot con ACL
  congelata (o `ADMIN`); enforcement **ogni op** (chiude TOCTOU).
- FD = capability; anti-confused-deputy: i servizi valutano il chiamante
  originario (canale propagato).
- Per-applicazione nativo: bucket privati, entitlement dichiarati
  (bucket+verbi), sandbox per subtree, re-attest agli update (mai
  silent-widen oltre i verbi concessi).
- Estensioni future senza rework: bearer token per condivisione esterna,
  macaroon/delega attenuata con zecca-server, ABAC temporale per recenza
  versioni. Gli UID di Strato 3 saranno un attributo in piu'.
- **Enforcement nel server, non nel provider**: `LocalFs::stat` non conosce
  il chiamante; la proiezione `mode`/`owner` e l'enforcement ABAC avvengono in
  `userfs` (Fase 17/37) attorno alla chiamata al provider. Se servisse
  contesto dentro il provider, si estende la trait, non si sposta
  l'enforcement.

## 7. Quota e subvolumi (T7)

- Subvolume = mount con budget (`quota_blocks`, `used` senza doppio
  conteggio dei blocchi condivisi; contatori nell'object tree, scrub a
  verifica).
- Nei bucket `block` la quota e' il budget del volume; durante il commit COW
  c'e' doppio conteggio transitorio (extent vecchi + nuovi), da non
  contabilizzare come `used` (§12).
- Enforcement al put (`ERR_NOSPACE` prima di allocare, mai transazioni
  mezze scritte); snapshot contro il budget del subvolume che li trattiene.

## 8. Metadati (xattr + creator)

- Per versione (immutabili): `{creator_app, creator_uid (=0), creator_sign
  (=0), tick, size}` — `0` = non misurato all'epoca, mai wildcard.
- Per oggetto (mutabili, bump `ctime` senza nuova versione): xattr
  `user.*` liberi + `sys.*` riservati; chiavi ≤ 64B, valori ≤ 1KB,
  totale ≤ 2KB inline (oltre → blob attributi, pattern overflow).
- `sys.content_hash` (riservato, BLAKE2s-256): hash del contenuto calcolato
  dal writer (userfs) al seal e verificato al load da `init` (N0: confronto
  con content-hash o re-hash dei byte, da allineare con ADR-0027/0037).
  Abilita dedup come **ottimizzazione futura** (piu' `object_id` → stesso
  extent, tracciato da refcount/GC); non e' identita' (quella resta
  `(volume_uuid, object_id)`). FNV-64 esplicitamente scartato qui:
  compleanno a 2^32 inaccettabile su binari TCB.
- `ctime` = ultima modifica metadati/ACL; gli xattr alimentano ABAC
  (filtri) e vector (filtri RAG).

## 9. Tool `arca` (T9)

- `create/list/get/put/rm/snap/quota/policy/scrub/stat/swap` — un binario
  dedicato (non builtin: gira col proprio canale e i propri bit).
- Auto-mount: mountpoint + `auto` sul volume, scan boot (`DISK_LIST`),
  cache solo hint, conflitti dichiarati (doppio mountpoint = secondo
  inattivo + log).
- Il tool non scavalca: valuta il canale originario; `grant` mai oltre il
  tetto del concedente.
- **Transizione di boot**: terzo drive `arca.img` opt-in (`ARCA_IMG=1`); si
  avvia sempre da FAT finche' ArcaFS non e' verificato (ArcaFS come volume
  secondario). `arca create` + iniezione nel volume; `run.sh` esteso per il
  terzo drive e per scegliere il boot volume. **Swap** solo a gate verde;
  FAT resta fallback.

## 10. Multi-device, rete, swap, vector (T10+)

- RAID (A6): mirror prima (stesso extent, due `device_idx`), stripe dopo;
  commit client-side; device-id stabili da A0.
- Rete (A8): su device-id + generazioni; `net_cookie` + replica_set futuro.
- Swap: extent tipo `SWAP` + oggetto dimensionabile dal demone (stessa
  primitiva alla base dello storage VM/blocco, §12); sensori
  `SYS_MEMINFO`/`statvfs`/RSS; solo anonimo in v1 (text = scarta-ricarica,
  page-cache e shm = futuri); pager track separato dopo A2.
- Vector: servizio userspace separato (track parallelo V1, mai nel FS);
  embedding come oggetti derivati (`derived_from`, `model:`); RAG =
  similarita + filtri xattr → UUID → `GET_ID`.

## 11. Accesso database (supporto nel FS, motore fuori)

Il motore database (vettoriale o altro, ispirato a Jigen ma scritto per
Velordor in Rust come servizio userspace) vive **fuori** dal FS. Qui solo
le primitive di supporto — niente logica di indici, query o embedding:

- Modello I/O: **esplicito + cache nel servizio** (niente porting
  mmap-based). Il servizio pinna le strutture hot in heap e pesca il
  resto con `GET_ID`; niente fault-path, niente pager, niente deadlock.
- `R_OBJ_MGET`: batch generazionale (una IPC, N UUID → N blob) per fan-out
  tipo HNSW; generico, non vettor-specifico (serve anche a ls -l massivi,
  backup, scrub). Implementazione con A2, quando il profiling lo chiede.
- `R_SYNC` con modi None/Group/PerWrite: flush esplicito a gruppi
  (checkpoint ogni N + `SaveChanges`-like); con COW il commit sposta solo
  il puntatore, quindi Group costa quasi zero.
- Batch atomico multi-oggetto (BEGIN/COMMIT oltre il singolo re-key):
  per transazioni tipo shrink-swap e commit multi-file; torn-tail =
  scarto via checksum come il WAL di riferimento.
- `fallocate` + hint `sequential`/`random`: preallocazione run append
  (contiguita', mai ENOSPC a meta') e dichiarazione pattern per
  allocatore A5 e cache (ingestion sequenziale, HNSW random).
- Crash-marker documentato come pattern: lock-file + generation +
  dirty-bit (reconcile all'apertura, mai fsck cieco).
- Subvolumi per area (content/vectors/index = 3 subvolumi con quota
  propria) + snapshot O(1) al posto delle copie di backup.
- mmap file-backed: **fuori spec**, rivalutato solo su profiling
  (richiederebbe fault-path kernel anti-deadlock dedicato).
- Perche' piu' veloce di un FS normale: niente doppia cache (disco→ring→
  app in un viaggio), append COW senza journal metadati sul path caldo,
  placement che ascolta gli hint, zero-copy estendibile ai client FS.

## 12. Storage per VM / block device (supporto nel FS, backend fuori)

Stesso pattern di §11: le primitive nel FS, la semantica di volume nel servizio.
Obiettivo: oggetti grandi usabili come dischi VM (zvol-like) senza appesantire
ArcaFS. Non serve al self-hosting: track parallelo, dopo A1–A8.

- **Opzione C (scelta)**: ArcaFS fornisce le primitive; l'esposizione a blocchi
  e la semantica zvol vivono in un servizio separato. Il FS resta nativo e
  generico; il volume e' un consumatore.
- **Tipo di bucket** (sostituisce la policy per-oggetto): `object`
  (versionato, default) vs `block` (volume, head unico mutabile, nessuna
  versione storica). Una VM = un bucket `block` = un volume: la policy si
  dichiara una volta, non per oggetto (vedi §1).
- **COW != versionamento**: il bucket `block` perde la *retention* delle
  versioni, non il COW. Gli extent restano copy-on-write per due ragioni:
  atomicita' del commit (scrivi nuovo, flip del root, libera i vecchi) e
  snapshot (i blocchi pinnati non si sovrascrivono). Gli snapshot sono
  l'unica retention del bucket `block`.
- **Prerequisito — transaction group (TXG)**: buffer write-back + commit
  periodico (e a richiesta). La testa dell'oggetto e' **mutabile**; versioni e
  snapshot solo ai confini di commit. E' il concetto che riconcilia versioning
  e block device (come le TXG di ZFS): senza, ogni write da 4K crea una
  versione e il volume vivo non regge (§1).
- **Primitive lato ArcaFS**:
  - oggetto grande con **mappa offset→blocco** (non la lista chunk sequenziale
    di §4), con buchi per lo sparso;
  - **COW per-extent** a blocco fisso (512/4K, negoziato e **scollegato** da
    `block_size` del nodo, §3);
  - **head unico mutabile** (bucket `block`); gli snapshot creano un root
    aggiuntivo ai confini di commit;
  - **reserve/`fallocate` garantito** (mai `ENOSPC` a meta' write) e
    **discard/punch-hole** per il TRIM del guest;
  - **flush** a modi (§11 `R_SYNC` None/Group/PerWrite) e hint
    `sequential`/`random`.
- **Backend fuori dal FS**: servizio userspace che presenta l'oggetto come
  **block device** (stile `userdisk`, canale dedicato), mappa settori→extent,
  gestisce discard, resize e thin provisioning.
- **I/O path**: bulk multi-frame/scatter-gather verso i ring del client
  (chiude il limite ~4000B di §4 e l'aperto in §14), zero-copy dove possibile.
- **Fuori scope v1**: mmap file-backed e `O_DIRECT` (gia' fuori spec §11);
  compressione delle immagini; dedup di immagini (solo `sys.content_hash`
  opzionale, §8).
- **Da misurare prima di promettere**: IOPS/latenza 4K random, write
  amplification, memoria cache, con TXG acceso/spento.

## 13. Fasi (ROADMAP 50+: P1–P5 + A1–A8 + V1 + B1 + N0 + L0/L1)

Numerazione ROADMAP (le lettere restano come alias di binario): 50–54 =
P1–P5, 55 = A1+N0, 56 = A2, 57 = L0/L1, 58+ = A3–A8/V1/B1 (numeri assegnati
all'avvio).

- **P1–P5 preparatorie OS-first** (prima di A1, gate verde ciascuna):
  P1 orologio (lettore CMOS `0x70/0x71` in userspace + endpoint `time` con
  epoch+tick, `mtime` veri via trait); P2 vocabolario disco (`DISK_LIST`/
  `DISK_INFO` + sonda TRIM capability-only); P3 durabilita' (`R_SYNC`
  None/Group/PerWrite + contratto + sensori `statvfs`/`SYS_MEMINFO`); P4
  misura bulk (bench round-trip-vs-dimensione, CAP single-source, zero cambi
  di formato: la decisione si prende sui numeri); P5 integrita' + attrezzi
  (BLAKE2s con cancelli in-place, `sys.content_hash`, `arca create`
  skeleton, `testsarca`, `arca.img` come terzo drive opt-in `ARCA_IMG=1`,
  boot default intoccato). Senza N0, A1 resta teoria (vedi §0).
- A1 (+N0 in coppia, mai da solo): singolo-device (format via `arca create`,
  negotiate, mount, R/W) whole-disk **e in partizione** (MBR riusando il parse
  esistente + parser GPT nuovo con `PartLoc` a u64, guardia protective-MBR
  `0xEE` mai dati); monta come provider `LocalFs` (`negotiate` su
  `magic="ACFS"` partition-relative, prima di `vfat`) e parte nella transizione
  multi-disco (boot da FAT, ArcaFS su `arca.img` terzo drive opt-in —
  whole-disk da P5, variante GPT in-partizione da A1 — secondario fino allo
  swap).
- A2: COW + snapshot/clone + GC (+ packing, + `R_OBJ_MGET`).
- A3: quota + subvolumi.
- A4: ACL/ABAC engine + tool policy.
- A5: device-awareness (`DISK_INFO`, TRIM, policy allocator, hint).
- A6: RAID. — A7: tool completo. — A8: rete.
- V1 (parallelo, mai nel FS): servizio vettoriale sopra §11.
- B1 (parallelo, mai nel FS): backend VM/block device sopra le primitive §12
  (richiede TXG); non serve al self-hosting.
- **Gate/testing**: `testsarca` (round-trip, dedup-check, snapshot/rollback,
  recovery), anti-rot (`docs/src/11-testing.md`, `06-syscalls.md`,
  `SUMMARY.md`, `run-tests.sh`, conteggi `AGENTS.md`). Gate corrente
  5/5 + 7/7 + 57/57. Criteri di swap: suite verde con i servizi caricati da
  ArcaFS e FAT di fallback funzionante.
- N0 (primo consumatore, precoce): `init` carica i servizi da ArcaFS per
  `object_id` (bucket `sys`), dual-mode con fallback FAT; vedi sotto.

### Primo consumatore nativo: `init` per `object_id`

`spawn_image` e' gia' **memory-based** (syscall 38): il kernel non tocca il
FS, la path vive solo in `init::spawn_file` (`libr::load_file` → `open/read`).
Caricare i servizi da ArcaFS e' quindi quasi tutto userspace.

- **MVP**: in `libr` un `obj_get(bucket, key) -> Vec<u8>` su `R_OBJ_GET`
  (chunking `RING_MAX_PAYLOAD`, bound 256 KiB); `SvcMeta.path` diventa
  `bucket/key`; `spawn_image` invariato; `disk`/`fs` restano embedded
  (storage-TCB: init non puo' caricarli per `object_id` prima che il FS
  esista).
- **Dual-mode (transizione)**: `init` prova il caricamento nativo da `sys`; a
  fallimento ripiega sulla path FAT. Fail-loud invariato; il ramo FAT si
  rimuove solo a gate verde.
- **Identita'**: confronto con il content-hash BLAKE2s-256 dell'oggetto
  (`sys.content_hash`, §8) o re-hash dei byte, al posto del solo manifest
  FNV; da allineare con ADR-0027/0037.
- **Payoff**: con `sys` come snapshot, caricare da `sys` **e'** l'update
  atomico e il rollback: `init` nativo e' di fatto il primo pezzo del
  sysimage manager.
- **Validazione**: boot completo con tutti i servizi da `sys` e gate
  5/5+7/7+57/57 verde; fallback FAT provato; nessuna regressione sul tempo di
  boot; un servizio con hash manomesso viene rifiutato.
- **Dipendenze**: A1 (format/mount/GET); il rollback vero arriva con A2.

## 16. Decisioni Fase 55 (A1+N0)

### 16.1 Parte 1 — Tag `R_OBJ_PUT`/`R_OBJ_GET` e chunking

- **Tag separati** (non unificato): `R_OBJ_PUT = 0x26`, `R_OBJ_GET = 0x27`.
  Numeri consecutivi a `R_GET_HASH = 0x25`. Single source in `syscall-numbers`.
- **Formato request GET**: `[tag:4][bucket_len:8][key_len:8][offset:8][count:8][payload]` dove il
  payload e' `[bucket]\0[key]\0` (null-terminated). Stateless: `offset` = byte
  di partenza nel blob, `count` = quanti byte si vogliono (max 4000).
- **Formato request PUT**: `[tag:4][bucket_len:8][key_len:8][size:8][payload]` dove
  il payload e' `[bucket]\0[key]\0[blob_data...]`. Blob opzionale (PUT vuoto =
  crea versione con 0 byte, solo per `touch`, non per `spawn_file`).
- **Formato response GET**: `[result:8][w1:8][blob_data...]` — `result` = size
  del blob o sentinella ERR_*, `w1` = 0, payload = dati.
- **Chunking esistente**: stesso schema di `read_fs`/`write_fs` (Fase 10.2).
  GET stateless: chunk da `RING_MAX_PAYLOAD = 4000B`, client fa loop con
  `offset += n` fino a raggiungere size totale (`result` del primo frame).
  PUT > 4000B → chunking multipli, ogni chunk = frame + `FS_NOTIFY` separata
  (come `write_fs`). PUT vuoto ha senso solo per `touch` (crea versione con
  0 byte), non per `spawn_file()` (un binario vuoto non si spawna).
- **Backend A1**: ramfs-like in-memory per bucket `sys` (hash map
  `bucket:key → blob`). Sufficiente per N0: init carica servizi da `sys`.
  Non implementa l'indice B+tree (T2) — quello arriva con A2.
- **Libr wrapper**: `obj_put(bucket, key, data) -> Result<u64, Error>` (ritorna
  size scritta), `obj_get(bucket, key, dst, max_count) -> Result<usize, Error>`.

### 16.2 Parte 2 — Parser GPT completo

- **PartLoc → enum**: `PartLoc::Mbr { start: u32, sectors: u32 }` (invariato) +
  `PartLoc::Gpt { start: u64, sectors: u64 }` (LBA48 come richiesto).
- **Guard protective-MBR**: tipo della prima voce di LBA0 (byte 450 =
  446+4, NON 446 che e' il boot flag) = `0xEE` → segnale GPT. Se presente,
  non chiamare mai `sniff_identity()` sul settore 0 (e' superblock ArcaFS o
  spazzatura, non BPB FAT).
- **GPT header a LBA1**: magic `EFI-part` (8B), versione, CRC32 (saltato per
  A1), current LBA, backup LBA (ignorato per A1), first usable LBA, last usable
  LBA, partition entries array offset, numero entry, size entry. Backup GPT
  header a fine disco ignorato per A1 (solo diagnostica futura).
- **128 entry**: full standard GPT, ogni entry 128B. Type GUID (16B), unique
  GUID (16B, ignorato), first LBA (u64), last LBA (u64), flags (ignorati per
  A1). Calcolo: `sectors = last - first + 1`.
- **Type GUID ArcaFS**: costante commentata placeholder in `part.rs` — non
  testiamo il match su disco in A1. Da registrare con UUID alias prima del
  rilascio.
- **Nomi partizioni GPT**: stesso namespace MBR (`sdXn`: `sda1`, `sda2`...).
  La logica di resolve in `nodes.rs` e' identica (per nome).
- **Integrazione server.rs**: dopo il whole-disk node, chiama
  `part::parse_mbr_or_gpt()` che: (1) legge LBA0, (2) se il tipo della prima
  voce (byte 450) == `0xEE` →
  `parse_gpt()`, (3) altrimenti → `parse_mbr()` (comportamento attuale).
- **locate() in nodes.rs**: estendere a gestire `PartLoc::Gpt { start, sectors }`
  dove i campi sono `u64` (LBA48). I calcoli di base/sectore restano gli stessi.

### 16.3 Parte 3 — N0: init dual-mode per `object_id`

**Modifica SvcMeta (`init/src/main.rs`):**
- Aggiungere campo opzionale `obj: Option<(&'static [u8], &'static [u8])>` dove
  il primo elemento e' il bucket (max 16B), il secondo la chiave (max 255B).
- Campo `path: Option<&'static str>` rimane per i servizi FAT legacy.
- Zero ambiguità: se `obj.is_some()` → nativo ArcaFS, `path` ignorato;
  altrimenti → FAT (comportamento attuale).

**Esempio di dichiarazione servizio:**
```rust
const SVC_USERCONSOLE: SvcMeta = SvcMeta {
    bin: b"userconsole",
    path: Some("/fat/bin/console.bin"), // legacy fallback
    obj: Some((b"sys", b"bin/userconsole.bin")), // nativo ArcaFS
    prio: 16,
    io: VGA_CURSOR_RANGES,
};
```

**Funzione `spawn_object()` (nuova):**
- Chiamata da `spawn_entry()` quando `meta.obj.is_some()`.
- Costruisce request frame `[R_OBJ_GET, bucket_len, key_len, offset=0, count=4000, payload]` dove
  payload = `bucket\0key\0` (null-terminated).
- Invia tramite `req_ring_write(FS_NOTIFY, ...)` + IPC al server ArcaFS.
- Legge response frame: `[size:8][w1:8][blob...]`. Se size > RING_MAX_PAYLOAD,
  loop chunked come `read_fs` (Fase 10.2): ogni chunk = round-trip con offset
  incrementale (`offset += n`, `count = min(4000, size - offset)`).
- Accumula i blob in un `Vec<u8>` (bound 256 KiB per A1: `SPAWN_IMAGE_MAX`).
- Ritorna `Some(Vec)` o `None` a errore. PUT vuoto non usato da init (solo
  `touch` futuro).

**Funzione `spawn_entry()` modificata:**
```rust
fn spawn_entry(meta: &SvcMeta) -> Option<i64> {
    match meta.obj {
        Some((bucket, key)) => {
            // 1. Prova ArcaFS nativo
            let img = match libr::obj_get(bucket, key) {
                Ok(v) if !v.is_empty() => v,
                _ => None, // fallimento → fallback FAT
            };
            if let Some(img) = img {
                return spawn_image_from_vec(&img, meta);
            }
            println!("[init] obj_get fallito, ripiega su FAT");
        }
        None => {}
    }
    match meta.path {
        None => spawn_child(meta.bin), // embedded (disk/fs)
        Some(_) => spawn_file(meta),   // FAT legacy
    }
}
```

**Verifica hash (`spawn_image_from_vec`):**
- Stesso pattern di `spawn_file()` attuale: controlla se `expected_hash(meta.bin)`
  ritorna Some, re-hash dei byte caricati con `libr::image_hash()`.
- Se mismatch → None (a boot = panic come prima; in restart = retry con hold).
- **Estensione BLAKE2s** (sessione dedicata): confronto con `sys.content_hash`
  dal manifest generato a build-time. L'hash BLAKE2s-256 viene calcolato su
  ogni oggetto al put (`arca put`) e memorizzato in xattr `sys.content_hash`.

**Libr wrapper `obj_get()`:**
```rust
pub fn obj_get(bucket: &[u8], key: &[u8]) -> Result<Vec<u8>, Error> {
    // Costruisce request frame, invia FS_NOTIFY, legge response ring
    // Chunking automatico se size > RING_MAX_PAYLOAD (loop come read_fs)
}
```

**Validazione N0:**
- Boot completo con tutti i servizi da `sys` e gate 5/5+7/7+57/57 verde.
- Fallback FAT provato: ogni servizio ha il ramo FAT di riserva.
- Nessuna regressione sul tempo di boot (ArcaFS in-memory, zero latency extra).
- Un servizio con hash manomesso viene rifiutato (test: init non lo spawna).

### 16.4 Parte 4 — Mount ArcaFS in partizione

**Superblock partition-relative:**
- LBA0 = primo settore del nodo (whole-disk o partizione GPT).
- LBA1 = shadow superblock (`LBA0 + 1`). Non collide con header GPT a LBA
  fisica 1 perché il nodo e' **partition-relative**: `base=0` punta all'inizio
  della partizione, non al disco fisico.
- Superblock identico a whole-disk (§3), ma tutti gli offset sono relativi al
  start della partizione.

**Negotiate (`mount.rs::negotiate()`):**
- Estensione del flow esistente (Fase 54): dopo `probe_arca(handle)` che legge
  LBA0 del nodo, match su `magic="ACFS"` → mount come `ArcaFs`.
- Per partizioni GPT: il nodo e' `sdXn` (es. `sda1`) con `PartLoc::Gpt { start,
  sectors }`. userdisk lo esporta come handle IPC; userfs risolve per nome
  (`DISK_RESOLVE` → handle) e legge LBA0 relativo.
- Order: ArcaFS prima di vfat (come gia' fatto per whole-disk). Il superblock
  ArcaFS ha magic + checksum che non collidono con BPB FAT.

**arca create:**
- Opzione `--in-partition <start_lba> <sectors>` (sostituisce `--whole-disk`):
  scrive superblock a LBA0 relativo (start della partizione) + shadow a LBA1.
- **Riconoscimento automatico**: se il nome dispositivo inizia con `sd[a-z][1-9]`
  (partizione) invece di `sd[a-z]` (whole-disk), arca create imposta automaticamente
  l'offset partition-relative senza bisogno di parametri appositi.
- Inizializza il transient set B+tree (Fase A1: solo hash map in-memory per
  `sys`, ma il disco e' pronto per l'estensione futura).
- **NON** inizializza GPT: se il disco non ha una tabella partizioni, arca create
  fallisce loud. La creazione del partition table e' un'operazione separata
  (strumento dedicato o script di provisioning).

**Integrazione con userdisk:**
- userdisk esporta nodi partizione GPT come `sdXn` (stesso namespace MBR).
- `DISK_INFO` per ogni nodo: tipo (`whole_disk`/`partition`), start LBA, sectors.
- Negotiate prova ArcaFS su ogni nodo; se superblock valido → mount, altrimenti
  tenta vfat.

## 14. Punti aperti (stima, non vincoli)

S1/S2 e extent minimo esatti (su dati P2, non a stima); dimensione esatta
del footer col tag128; orphan-scan vs journal con snapshot multipli;
formato entitlement; threshold transient set (solo fino ad A2, poi marker).
Chiusi dalle P: `DISK_LIST` (P2), wall-clock oltre i tick (P1), framing
multi-frame per `R_OBJ_MGET` oltre 4000B (P4 misura, decisione sui numeri).

Aggiunti (dalle decisioni di integrazione):

- Semantica esatta di `write` a offset e `ftruncate` (versioning, costo COW,
  interazione con `nlink`/versioni trattenute).
- Dove e con quale algoritmo calcolare `sys.content_hash` (dedup futura).
- Minimo POSIX del toolchain da verificare nel porting: `symlink`/`link`,
  `utimes`, `chown`.
- Dove sta la proiezione `mode`/`owner` ABAC (confermato server; valutare se
  il provider avra' mai bisogno di contesto).

Storage VM/blocco (§12):

- Granularita' del blocco COW (512 vs 4K) e sua negoziazione, scollegata da
  `block_size`.
- Policy TXG: dimensione del buffer, soglia/timer di commit, interazione con
  `R_SYNC`.
- Contabilita' di reserve e discard rispetto a quota (`used` senza doppio
  conteggio) e allo scrub.
- Forma della mappa offset→blocco (livelli, pagine, costo di aggiornamento).
- Protocollo del backend block device (canale dedicato) e framing bulk
  multi-frame oltre i ~4000B.
- Compressione delle immagini: fuori v1, da rivalutare.
- Conversione di un bucket `object` → `block` (GC delle versioni) e ritorno.
- Retention dei blocchi pinnati dagli snapshot nei bucket `block` e doppio
  conteggio quota durante il commit.

## 15. Logging (specifica aggiuntiva, graduale)

Il FS fornisce le primitive, un servizio userspace separato fa il log
(stesso pattern di §11/§12: niente query nel FS, mai).

- L0 (prima di A1, su FAT): convenzione `/var/log` + rotazione nel servizio;
  early-boot sempre su seriale+dmesg (timestamp in tick, best-effort).
- L1 (dopo A2, nativo): bucket `log` tipo `object`, chiavi
  `<sorgente>/<giorno>/<seq>`; append con seal periodico (stile TXG Group
  commit) + `R_SYNC` Group al seal; retention via snapshot+GC (richiede A2);
  quota sul subvolume; niente query nel FS (il servizio indicizza fuori).
- Prerequisiti: P1 (timestamp veri — senza wall-clock i log non sono log),
  P3 (`R_SYNC` + contratto di stabilita'), A2 (retention senza GC e' solo
  accumulo).
- Formato record e policy di seal/retention: dettaglio in A2, non qui.

## 17. Fase 56.1 — versioni in RAM (decisioni)

Primo passo A2: semantica versionata senza disco (il B+tree on-disk e' 56.2).

- **Ogni PUT = nuova versione**: offset 0 = versione da zero (compat A1);
  offset > 0 = clone della head con range patchata (ogni chunk di upload e'
  una versione COW — accumulo corretto + storia completa).
- **Identita'**: `object_id` monotonico da 1, mai riusato (F2); indice
  inverso id→key; `GET_ID`/`STAT_ID` per UUID, `STAT` per (bucket,key)
  ritorna `(id, size, nversioni, mtime)`.
- **Snapshot per-bucket con pin a COPIE** (non refcount): DELETE/trim non
  invalidano mai uno snapshot. Scelta dichiaratamente transitoria: con
  extent condivisi (56.2) le copie diventano refcount — tradeoff RAM vs
  semplicita' a scala gate, documentato non nascosto.
- **Rollback = nuova head clonata** (mai truncate: anche il rollback resta
  in storia); solo stesso bucket (cross-bucket → `ERR_INVALID`).
- **Clone = nuovo bucket, nuovi id** (una versione per chiave).
- **GC 56.1 = retention (8, `VERSION_RETAIN`) + drop a snapshot-delete**.
  N=8: abbastanza storia per rollback utili, poco costo in RAM a scala gate;
  si rivaluta con i numeri d'uso (A3 ha la quota vera).
- **Tag** `R_SNAP_CREATE/DELETE/ROLLBACK/CLONE` 0x28–0x2B,
  `R_OBJ_GET_ID/STAT_ID/DELETE/STAT` 0x2C–0x2F; diritti: nessun bit (come
  `R_OBJ_*`, ABAC e' A4). Reply STAT a due registri + frame dedicato
  (pattern `R_PIPE_CREATE`).
- **Errori**: `INVALID` oltre bound nomi / bucket mismatch, `NOTFOUND`
  assente — mai dati inventati. Frame SEMPRE scritto anche a errore
  (disciplina anti-desync della Fase 55); il client consuma prima di
  controllare i registri (mai letture oltre l'header).

## 18. Fase 56.2a — formato on-disk + allocatore (decisioni)

- **Blocco 3584 B = 7 settori** (1 op `DISK_*` esatta), partition-relative;
  blocco 0 = superblock + shadow + header-estensione (settori 2-6, i primi
  1024 B intoccati); blocco 0 mai allocato. Header-estensione 56 B con
  checksum (free_head, high_water da 1, next_id/next_snap per 56.2b, flags
  con DIRTY per il commit). Nodo: magic + type (RAW opaco in 56.2a) + gen
  (0) + payload 3560 + FNV (tag128 crypto rimandato ad A7).
- **`arca create` inizializza il volume** (header-ext + root vuota + ROOT=1
  con checksum; shadow sincronizzato al format). `volume_uuid` auto da OS
  RNG (`--uuid` solo override deterministico).
- **Debug via UN tag** (`R_ARCA_DEBUG` + sub-op): scaffold gate su volume di
  scratch, gating di policy in A7. Mai nel percorso R_OBJ_* (in-RAM).
- **Casa `arcafs/`**: tag/wire/formato condivisi guest/host; i wrapper IPC
  restano in `libr` (evita il ciclo `libr`↔`arcafs`); `libr` riesporta.
- **Lezione stack**: il loop userfs gira su 16 KiB con buffer 4K nei
  handler — un ritorno by-value da 3.5 KiB (+inline) sfonda la guardia
  (osservato: #PF deterministico a ogni boot). Regola: payload grandi in
  `Box`, handler pesanti `#[inline(never)]`, mai array KiB per-valore nel
  loop. `userfs.bin` al 90% di `SPAWN_IMAGE_MAX`: budget codice contato
  per 56.2b.

## 19. Piano 56.2b/56.2c (A2 su disco)

Stato dopo 56.2a: formato blocchi, allocatore freelist, nodi opachi con
checksum, `R_ARCA_DEBUG`, casa `arcafs/`, suite 27/27. Resta: portare la
semantica 56.1 (versioni, snapshot, rollback, clone, retention) sui blocchi
e accendere la persistenza. Due passi con gate separati.

### 56.2b — B+tree + commit (il passo grosso)

- **Nodi tipati**: LEAF (coppie key→valore) e INTERNAL (separatori + figli)
  sopra il contenitore 56.2a (header magic+type+gen, payload 3560, footer
  FNV). Chiavi: `object_id` u64 nel primary (8 B, confronto numerico),
  `(bucket,key)` a lunghezza esplicita nel secondary. Foglie con overflow
  per chiavi lunghe (record overflow linkati, mai tabelle fisse — regola
  §0); discesa con separatori corti.
- **Valori**: primary UUID → `{version_head, ...}`; le versioni sono record
  a catena (nuova versione = nuovo record + rewrite del puntatore head,
  mai overwrite — COW anche in 56.2b); secondary → UUID + stat
  denormalizzata (size, mtime, version_head) per `stat` senza primary.
- **Snapshot con extent condivisi**: le copie 56.1 diventano pin con
  refcount sul `refcount_root` del superblock (campo già riservato);
  DELETE/trim decrementano, lo zero libera. Il refcount vive nel commit
  come il resto (crash-safe per costruzione, mai file separato).
- **Allocatore**: freelist 56.2a + hint di placement (zone/co-location soft,
  A5 cambia politica); `alloc_hint` del superblock come seed.
- **Commit**: shadow superblock + flip di `generation`; DIRTY bit acceso
  durante la mutazione, spento al flip. Niente journal (decisione §3
  confermata). Ordine write: nodi nuovi → refcount → header-ext → shadow
  → flip superblock; a ogni passo interrotto la generazione vecchia resta
  valida (crash = generazione vecchia + orphan-GC in 56.2c).
- **Cache nodi**: write-through in-heap, LRU, cap iniziale 256 nodi
  (~1 MB, confermato in 56.2a); regola stack §18 vale per tutto il path
  (nodi in `Box`, mai per-valore nel loop). Tuning coi numeri, non a stima.
- **Vincolo binario**: `userfs.bin` al 90% di `SPAWN_IMAGE_MAX` — budget
  codice contato: niente duplicazioni (riuso `arcafs::format`), `opt z`;
  se sfora si sposta codice, non si alza il bound senza ADR.
- **Vittoria 56.2b**: tutta la semantica 56.1 passa identica ma su disco
  (stessi assert testsarca, backend diverso: il backend mem di 56.1 resta
  come oracolo di confronto); suite estesa con split/merge forzati
  (bulk insert oltre la capacità foglia) e crash a metà commit simulato
  (kill userfs durante PUT pesanti → al remount generazione vecchia
  intatta).
- **Rischi noti**: split/merge con overflow record (il caso che rompe i
  B+tree fatti in casa); refcount vs snapshot-delete concorrente al commit
  (ordine write sopra); heap userfs sotto churn (VERSION_RETAIN=8 resta).

> **Stato 56.2b (chiuso, gate 5/5+7/7+32/32+58/58)**: vittoria conseguita con
> tre deviazioni dichiarate dal piano sopra — (1) backend UNICO su disco, il
> mem 56.1 e' rimosso (era transitorio per §17; ~19 KiB oltre
> `SPAWN_IMAGE_MAX` non lasciano scelta) e l'oracolo e' nei test host
> `arcafs` (`MemStore`, 9 test), non in un doppio backend guest; (2) cache
> nodi LRU deferita al tuning coi numeri (correttezza prima, I/O diretto);
> (3) secondary root persistita in superblock `alloc_hint` (la root SI SPOSTA
> a ogni split: il blocco 2 fisso dava chiavi post-split invisibili al
> remount — il commit persiste tutte e tre le radici). Commit per-op
> (header-ext + shadow + flip), non TXG batchato (TXG resta prerequisito §12
> per i bucket `block`, non per l'object store). Resta 56.2c.

### 56.2c — Recovery + sys-dal-volume (CHIUSA, gate 5/5+7/7+33/33+58/58)

> Deviazioni dal piano dichiarate: (1) GC a **ogni** load-bind, non solo con
> DIRTY (superset: deterministico senza dipendere dal timing del kill); (2) la
> tabella snapshot si persiste dopo il load iniziale, non e' ricostruita dalla
> walk (un pin non lascia tracce nel primary oltre al refcount); (3) il
> puntatore meta vive in `alloc_hint` [108..116) riusando coda-mountpoint
> (clampato a 56 B), niente bump di versione formato per 8 byte; (4) marker
> dir persistenti confermati a 56.3 (§5 vs §19 risolto).

- **Mount/recovery**: superblock valido → orphan-GC (raggiungibili dai 3
  alberi + meta + catene overflow, meno freelist/live/blocco 0 → freelist;
  un solo commit chiude anche DIRTY); remount = reload completo = **test di
  crash deterministico** (niente reboot nel gate: kill userfs + remount +
  snapshot sopravvissuto USABILE via rollback). La guardia live e' esclusa
  dalla GC (leftover RAW mai sganciati = leak sicuro, mai double-push).
- **Tabella snapshot persistente**: blocco meta `TREE_META` (`snap/<sid:8>`
  → bucket+tick+(uuid,seq,seckey)*), puntatore in `alloc_hint`; meta
  corrotta/assente = mount senza snapshot loud (dati intatti), `next_snap`
  resta da header-ext (F2). La GC non dipende dalla tabella (i pin non
  sganciano mai i record dal primary).
- **sys-dal-volume**: auto-bind ArcaFS all'avvio (scan magic-driven) PRIMA
  del READY; `seed_sys` dal volume, seed solo delle chiavi assenti (i dati
  persistono nei commit, niente versioni duplicate). Fallback FAT se volume
  assente o chiave assente (dual-mode invariato, init intoccato). Con
  `ARCA_IMG=0` gli op nativi danno errore loud e la suite resta adattiva.
- **Reboot reale** (manuale, fuori gate): generazione N montata, snapshot
  sopravvissuto, servizi da `sys` — prova finale prima di dichiarare la
  vittoria 56 ("rollback vero; retention log implementabile", sblocco 57).
- **Fuori scope 56** (confermato): packing S1/S2, `R_OBJ_MGET`, marker dir
  persistenti (56.3); quota/subvolumi (A3); tag128 crypto (A7); loader EFI.

### Gate e docs per passo

- Suite `testsarca` cresce a ogni passo (stessi numeri solo se la semantica
  e' identica; nuovi assert per split/commit/recovery); `usertests`
  invariato salvo attori concorrenti necessari; gate e checklist anti-marcio
  invariati (`11-testing.md`, `07-ipc.md` se nuovi tag, `run-tests.sh`,
  conteggi `AGENTS.md`, `ROADMAP.md`, cronologia).
