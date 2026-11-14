# journal: a segmented event journal with snapshots and an inventory on top

journal is an append-only event log stored as segment files in one directory. It numbers records, survives a crash in the middle of a write, replays records, keeps snapshots, and compacts old segments. A small inventory state machine is rebuilt from the latest snapshot plus the records after it. It is a Go library with a command line. The project will be checked by tests written against the interface below, and those tests read and damage the files directly, so the file formats must match exactly.

## Module and packages

Module path `example.com/journal`, `go 1.22` in go.mod, standard library only. Packages: `record`, `journal`, `inventory`, `cli`. A thin `cmd/journal/main.go` may call `cli.Run(os.Args[1:], os.Stdout, os.Stderr)` and exit with its result.

## Package record

```go
type Record struct { Seq uint64; Type string; Payload []byte }
var ErrShort, ErrInvalid, ErrChecksum, ErrType error // distinct sentinel errors
func Encode(r Record) ([]byte, error)
func Decode(b []byte) (Record, int, error) // the record at the start of b and its size in bytes
```

Binary layout, all integers big-endian. Let BODY be: Seq as 8 bytes, the length of Type as 2 bytes, the bytes of Type, then the bytes of Payload. A record is: the length of BODY as 4 bytes (L), the CRC-32 (IEEE, as `hash/crc32.ChecksumIEEE`) of BODY as 4 bytes, then BODY. So a record takes 8 + L bytes and L is at least 10. `Encode` returns `ErrType` when Type is empty or longer than 65535 bytes. `Decode` ignores bytes after the record and checks in this order: fewer than 8 bytes gives `ErrShort`, L below 10 gives `ErrInvalid`, fewer than 8 + L bytes gives `ErrShort`, a CRC mismatch gives `ErrChecksum`, a type length of 0 or larger than L − 10 gives `ErrInvalid`. A decoded Payload is never nil (empty payload gives an empty slice).

## Package journal

```go
var ErrCorrupt, ErrClosed, ErrCompacted, ErrNoSnapshot, ErrBadSnapshot error // distinct sentinel errors
type Options struct { SegmentSize int64 } // 0 or less means 1048576
type Journal struct { /* unexported */ }
func Open(dir string, opts Options) (*Journal, error)
func (j *Journal) Append(typ string, payload []byte) (uint64, error)
func (j *Journal) LastSeq() uint64
func (j *Journal) FirstSeq() uint64
func (j *Journal) Segments() []uint64
func (j *Journal) Replay(from uint64, fn func(record.Record) error) error
func (j *Journal) Compact(upTo uint64) (int, error)
func (j *Journal) SaveSnapshot(seq uint64, data []byte) error
func (j *Journal) LatestSnapshot() (uint64, []byte, error)
func (j *Journal) Close() error
```

Errors may be wrapped with more detail, so tests use `errors.Is`.

Files. A segment file is named by the sequence number of the first record it holds (or would hold) as 20 decimal digits plus `.log`, for example `00000000000000000001.log`, and holds encoded records back to back. A snapshot file is named `snapshot-` plus the sequence number as 20 digits plus `.snap` and holds the CRC-32 (IEEE, 4 bytes big-endian) of the data followed by the data. Other files in the directory are ignored.

`Open` creates the directory if needed (it may hold no segment at all) and checks every segment in ascending order. Within a segment the records must have the sequence numbers name, name+1, name+2 and so on, and each segment's name must be one more than the last sequence number of the segment before it (an empty segment's last sequence number is its name minus 1). A segment named 0 or any break in this numbering is `ErrCorrupt`. A record that fails to decode is handled like this: if it is in the last segment and its declared end (its offset + 8 + L, where L is read from its first 4 bytes, or the end of the file when fewer than 8 bytes are left) is at or beyond the end of the file, it is a torn write and the file is truncated to the offset where that record starts. Any other record that fails to decode is `ErrCorrupt`, so a damaged record followed by more bytes never gets dropped. When `Open` returns `ErrCorrupt` it has not changed any file.

- Sequence numbers start at 1 and continue after reopening. `LastSeq` is the last record's number, or the last segment's name minus 1 when that segment is empty, or 0 when there is no segment.
- `FirstSeq` is the first segment's name, or 1 when there is no segment. `Segments` lists the segment names ascending.
- `Append` gives the record the number `LastSeq()+1` and writes it to the file at once (no buffering). It starts a new segment, named by the new record's number, when there is no segment yet, or when the last segment is not empty and adding the record would make it larger than SegmentSize. So a record larger than SegmentSize sits alone in its segment. A type rejected by `record.Encode` returns `record.ErrType` and writes nothing.
- `Replay` calls `fn` for each record with `Seq >= from` in order (`from` 0 counts as 1). If `from` is below `FirstSeq()` it returns `ErrCompacted` and calls nothing. An error from `fn` stops the replay and is returned as is.
- `Compact` deletes, oldest first, each segment that is not the last one and whose records all have `Seq < upTo` (that is, the next segment's name is at most upTo), and returns how many it deleted. Snapshots are kept.
- `SaveSnapshot` returns `ErrBadSnapshot` unless 1 ≤ seq ≤ `LastSeq()`. Saving the same seq again replaces the file. `LatestSnapshot` returns the snapshot with the highest sequence number among those whose file is at least 4 bytes with a matching CRC (damaged snapshots are skipped), or `ErrNoSnapshot` when there is none.
- After `Close`, every method except `LastSeq`, `FirstSeq` and `Segments` returns `ErrClosed`, including a second `Close`.

## Package inventory

```go
var ErrMalformed = errors.New("inventory: malformed")
var ErrInsufficient = errors.New("inventory: insufficient stock")
type Item struct { SKU string; Qty int }
type Inventory struct { /* unexported */ }
func New() *Inventory
func (inv *Inventory) Apply(r record.Record) error
func (inv *Inventory) Stock(sku string) int
func (inv *Inventory) Items() []Item // sorted by SKU, only quantities above 0, empty (not nil) when none
func (inv *Inventory) Encode() []byte
func Decode(data []byte) (*Inventory, error)
type Store struct { /* unexported */ }
func Load(j *journal.Journal) (*Store, error)
func (s *Store) Inventory() *Inventory
func (s *Store) Add(sku string, qty int) (uint64, error)
func (s *Store) Remove(sku string, qty int) (uint64, error)
func (s *Store) Snapshot() (uint64, error)
```

A SKU is one or more of the characters `A-Z a-z 0-9 - _ .`. An event payload is `SKU QTY`: the SKU, one space, and QTY as a positive decimal integer of digits only. `Apply` handles type `add` (stock rises by QTY) and `remove` (stock falls by QTY). Any other type or a payload not in that form gives an error wrapping `ErrMalformed`. A `remove` larger than the current stock gives an error wrapping `ErrInsufficient`. A failed `Apply` changes nothing. A SKU whose stock reaches 0 disappears from `Items`.

`Encode` writes one line `SKU QTY\n` per item in `Items` order (empty output for an empty inventory). `Decode` accepts that form with lines in any order and returns `ErrMalformed` (possibly wrapped) for anything else, including a missing final newline or a SKU listed twice.

`Load` takes `LatestSnapshot`. With a snapshot at S it decodes it and replays from S+1, without one it starts empty and replays from 1. Any error (a bad snapshot, `ErrCompacted`, an `Apply` error) is returned. `Add` and `Remove` check their arguments first (a bad SKU or a qty below 1 returns `ErrMalformed` itself), `Remove` then returns `ErrInsufficient` itself when the stock is too low, and in both failure cases nothing is appended. Otherwise they append the event (types `add` or `remove`, payload as above), apply it, and return its sequence number. `Snapshot` saves `Encode()` as the snapshot at `LastSeq()` and returns that number (it fails with `ErrBadSnapshot` on an empty journal).

## Package cli

```go
func Run(args []string, stdout, stderr io.Writer) int // args exclude the program name
```

Every command opens the journal in DIR with default options and closes it at the end.

- `append DIR TYPE PAYLOAD`: appends the raw record and prints its sequence number.
- `dump DIR [FROM]`: prints each record from FROM (default 1) as `SEQ TYPE PAYLOAD` where PAYLOAD is formatted with Go's `%q`, for example `3 add "apple 5"`.
- `compact DIR UPTO`: prints `removed N`.
- `add DIR SKU QTY` and `remove DIR SKU QTY`: load the Store, call Add or Remove, print the sequence number.
- `stock DIR`: loads the Store and prints `SKU QTY` per item in `Items` order (nothing when empty).
- `snapshot DIR`: loads the Store, calls Snapshot, prints `snapshot SEQ`.

Every output line ends with `\n`. Exit 0 on success. Any error from the journal or inventory is written to stderr as `error: ` plus the error text and exits 1 (for example `error: inventory: insufficient stock`). An unknown command, a wrong number of arguments, or a FROM, UPTO or QTY that is not a decimal integer of digits only prints a line starting with `usage:` to stderr and exits 2, without touching DIR.

## Constraints

Go 1.22 or later, standard library only, no network. Your own tests may live anywhere in the module except files named `hidden_*_test.go`, which are reserved for the checking tests.
