# ADR-0039: Logging L1 nativo (Fase 57)

**Status**: Implemented (Fase 57 — gate 5/5 + 7/7 + 40/40 + 58/58).

## Context

`arcafs.md` §15 prevedeva il logging in due passi: L0 (convenzione
`/var/log` su FAT + rotazione nel servizio, "prima di A1") e L1 (bucket
`log` nativo, "dopo A2"). L0 non e' mai stato implementato; A2/56 e'
chiusa (B+tree COW + commit per-op + snapshot persistenti + orphan-GC +
`sys`-dal-volume). I prerequisiti §15 sono tutti chiusi: P1/orologio
(Fase 50, `Service::Time`), P3/durabilita' (Fase 52, `R_SYNC` +
contratto), A2/retention (Fase 56).

## Decision

**Solo L1 nativo; L0 cancellato senza implementarlo.** La ragione di L0
(ArcaFS non esisteva) e' estinta: costruire rotazione-su-FAT oggi
significherebbe scrivere codice usa-e-getta contro l'orizzonte
dichiarato (sganciarsi dal FAT).

### 57.0 — Servizio `userlog` (`Service::Log = 10`), primo dopo init

Nuovo binario userspace, spawnato da init **in parallelo a disk e prima
di fs** (il chicken-egg e' morto per costruzione), supervisionato con
restart. All'avvio **zero dipendenze**: niente FS, niente `Time` — solo
`get_ticks` (syscall diretta). Zero syscall nuove, zero tag ArcaFS nuovi,
zero bit di diritto nuovi:

- Protocollo client→`userlog` (solo registri + ring LOG propri del
  client, stampo `TIME_*`/`FS_BUF_REG`): `LOG_REG` (register-only dei
  phys ring; reply `(hash_bucket, 0)`), `LOG_APPEND` (frame nel proprio
  ring + notify; reply `(seq, durable)`), `LOG_READ` (payload
  `[giorno:8][seq:8]` sul bucket proprio, `seq=0` → latest; risposta nel
  response ring + reply `(len, seq)`), `LOG_SEAL` (snapshot esplicito;
  reply `(snap_id, 0)`), `LOG_STATS` (registri + frame
  `[durable:8][last_seal:8]`), `LOG_FLUSH` (solo parent: handshake FS +
  backdate/re-key + riversamento incrementale; reply `(0,0)`).
- Ogni APPEND e' **sync-su-RAM** (µs): il disco non e' mai nel percorso
  del chiamante. Il client condivide gli anelli col FS in sequenza (una
  sola coppia per processo — il kernel mappa ogni coppia sulle stesse VA);
  cancello leggero senza handshake FS (`fs_light_gate`: alloca se serve,
  rifiuta su fork/async-in-volo invece di corrompere).
- Backend volume: `R_OBJ_PUT/GET` esistenti nel bucket `log` + `!idx`
  per-bucket-giorno (max seq, per la latest dopo un restart) + `R_SNAP_*`
  per seal/retention. Niente `R_SYNC` al seal: commit per-op.
- Diritti: riuso del canale obj (nessun bit nuovo: `RIGHTS_ALL` e
  `TEST_POLICY` intoccati — lezione Fase 52).

### 57.1 — Formato record e chiavi (single source in `libr::log`)

Record `[tick:8][epoch:8][level:1][taglen:1][tag][msg]`, tick ed epoch
messi dal **server**; bound tag 1..=32 B, msg 1..=1024 B (un frame).
Livelli `INFO/WARN/ERR` = convenzione al bordo, mai kernel.

Chiavi `<hash16hex>/<giorno8hex>/<seq16hex>` dove hash e' il `peer_info`
(FNV-1a dell'ELF) del chiamante: **il bucket e' attribuito dal server,
mai dichiarato** (spoofing impossibile per costruzione; policy umana sui
nomi rimandata ad A4). Il `tag` resta hint leggibile a zero semantica.

### 57.2 — Fasi RAM → volume (FLUSH di init)

- Nascita→FLUSH: solo RAM (coda 128, evict-oldest contato; epoch 0,
  giorno `00000000` se `Time` non c'e' ancora).
- `LOG_FLUSH` (init, dopo fs+time): handshake FS (primo contatto FS di
  sempre), **backdate + re-key in RAM** (`epoch = now − (tick_now −
  tick_rec)/100`, giorno vero, seq ridati; il giorno-0 non raggiunge mai
  il disco — la RAM e' mutabile, il disco e' per sempre), riversamento
  incrementale (4/giro di loop, mai bloccare gli APPEND), dual-write.
- Senza `Time` alla FLUSH: degrado dichiarato (epoch-0 persistono; mai
  riscrittura del passato). Senza volume: RAM-only + retry a ogni append
  (self-healing dopo restart di userfs). Restart di userlog: re-bind
  opportunistico se `Fs` e' registrato (lookup singolo, mai attesa);
  l'indice si ricostruisce da `!idx`; le ere si sovrappongono come
  versioni (mai corruzione, GET legge la latest).

### 57.3 — Boot parallelo e non-POSIX

- init spawna `{log, disk}` in parallelo + `wait_any` sui READY (morte
  pre-ready = fail loud nominativo); `fs` resta dopo `disk` (il mount
  aspetterebbe comunque il disco: parallelizzarlo spenderebbe il bound
  HELLO da 5 s per guadagno ~0). I milestone di boot vanno a `log()`
  diretti dal primo spawn (niente boot-buffer: userlog e' gia' su).
- Regola non-POSIX (verifica di fase: `rg "open\(|read_fs|write_fs|::mount"
  userland/log/src libs/libr/src/log.rs` = zero fuori commenti): userlog
  e client usano solo API native (`R_OBJ_*`, `R_SNAP_*`, `TIME_NOW`).
- Lo storage-TCB non chiama mai `userlog` (anti-ciclo); il kernel resta
  su seriale per disegno; il pre-boot resta anche su seriale.

## Consequences

### Positive

- userfs intoccato (budget `SPAWN_IMAGE_MAX`); kernel: 2 righe meccaniche
  (`service_from_disc` + `service_name`, zero semantica).
- Restart-safe per costruzione (versioni + `!idx`); boot senza deadlock
  (log indipendente da fs per disegno, provato dall'ordine di spawn).
- Effetto collaterale: cancello `fs_gate` aggiunto agli `obj_*` di `libr`
  (prima un primo uso senza FS faultava su VA non mappate — #PF osservato
  e fixato in fase).

### Negative

- Senza volume niente persistenza (RAM-onlyproduttivo con `ARCA_IMG=0`).
- 2 PUT per append (dato + `!idx`): costo dichiarato, rate log ≪ rate FS.
- Niente LIST globale (manca sul wire): solo own-bucket; inventario
  operatori al tool A7. Lettura record altrui impossibile in 57.

### Neutral

- Drop ammesso e contato (code IPC da 8 slot, Fase 13); `LOG_STATS`
  espone `(appended, evicted, durable, last_seal)`.
- `LOG_FLUSH` accettata solo dal parent (pid 1): gli altri vedono ERR.

## Alternatives Considered

- **L0 su FAT prima**: scartato — codice da buttare, contro l'orizzonte.
- **Engine ArcaFS dentro userlog su partizione dedicata**: scartato per
  57 — indipendenza totale ma secondo volume da formattare/gestire e
  protocollo DISK multi-client da estendere (ring DISK condivisi);
  rivalutabile se userfs diventera' collo di bottiglia per i log.
- **Segmenti raw append-only custom**: scartato — formato in piu' da
  mantenere contro B+tree gia' pagato.
- **Rotazione dentro userfs / ognuno per se'**: scartati (budget binario,
  N implementazioni, sprawl diritti) — vedi piano iniziale.
- **`R_SYNC(GROUP)` al seal**: scartato — no-op su commit per-op.
