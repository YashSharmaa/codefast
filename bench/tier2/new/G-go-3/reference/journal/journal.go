// Package journal is an append-only, segmented event log with snapshots.
package journal

import (
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"example.com/journal/record"
)

var (
	ErrCorrupt     = errors.New("journal: corrupt")
	ErrClosed      = errors.New("journal: closed")
	ErrCompacted   = errors.New("journal: records compacted")
	ErrNoSnapshot  = errors.New("journal: no snapshot")
	ErrBadSnapshot = errors.New("journal: bad snapshot sequence")
)

// DefaultSegmentSize is used when Options.SegmentSize is zero or negative.
const DefaultSegmentSize = 1 << 20

// Options configure a journal.
type Options struct {
	SegmentSize int64
}

// Journal is an open journal directory.
type Journal struct {
	dir      string
	segSize  int64
	segments []uint64
	lastSeq  uint64
	tailSize int64
	closed   bool
}

func segName(first uint64) string { return fmt.Sprintf("%020d.log", first) }

func snapName(seq uint64) string { return fmt.Sprintf("snapshot-%020d.snap", seq) }

// parseName returns the number in a name of the form prefix + 20 digits + suffix.
func parseName(name, prefix, suffix string) (uint64, bool) {
	if !strings.HasPrefix(name, prefix) || !strings.HasSuffix(name, suffix) {
		return 0, false
	}
	digits := name[len(prefix) : len(name)-len(suffix)]
	if len(digits) != 20 || strings.Trim(digits, "0123456789") != "" {
		return 0, false
	}
	n, err := strconv.ParseUint(digits, 10, 64)
	return n, err == nil
}

func (j *Journal) path(name string) string { return filepath.Join(j.dir, name) }

// Open opens or creates the journal in dir and recovers its tail.
func Open(dir string, opts Options) (*Journal, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, err
	}
	j := &Journal{dir: dir, segSize: opts.SegmentSize}
	if j.segSize <= 0 {
		j.segSize = DefaultSegmentSize
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	for _, e := range entries {
		if n, ok := parseName(e.Name(), "", ".log"); ok && !e.IsDir() {
			j.segments = append(j.segments, n)
		}
	}
	sort.Slice(j.segments, func(a, b int) bool { return j.segments[a] < j.segments[b] })
	for i, first := range j.segments {
		if first == 0 || (i > 0 && first != j.lastSeq+1) {
			return nil, fmt.Errorf("%w: segment %d out of sequence", ErrCorrupt, first)
		}
		if err := j.scan(first, i == len(j.segments)-1); err != nil {
			return nil, err
		}
	}
	return j, nil
}

// scan checks one segment, truncating a broken final record of the last one.
func (j *Journal) scan(first uint64, last bool) error {
	p := j.path(segName(first))
	data, err := os.ReadFile(p)
	if err != nil {
		return err
	}
	expect := first
	off := 0
	for off < len(data) {
		r, n, err := record.Decode(data[off:])
		if err != nil {
			end := int64(len(data))
			if len(data)-off >= record.HeaderSize {
				end = int64(off) + record.HeaderSize + int64(binary.BigEndian.Uint32(data[off:off+4]))
			}
			if !last || end < int64(len(data)) {
				return fmt.Errorf("%w: segment %d offset %d: %v", ErrCorrupt, first, off, err)
			}
			if err := os.Truncate(p, int64(off)); err != nil {
				return err
			}
			break
		}
		if r.Seq != expect {
			return fmt.Errorf("%w: segment %d: sequence %d, want %d", ErrCorrupt, first, r.Seq, expect)
		}
		expect++
		off += n
	}
	j.lastSeq = expect - 1
	j.tailSize = int64(off)
	return nil
}

// LastSeq returns the sequence number of the newest record, 0 when empty.
func (j *Journal) LastSeq() uint64 { return j.lastSeq }

// FirstSeq returns the sequence number the oldest kept segment starts at.
func (j *Journal) FirstSeq() uint64 {
	if len(j.segments) == 0 {
		return 1
	}
	return j.segments[0]
}

// Segments returns the first sequence numbers of the segment files, ascending.
func (j *Journal) Segments() []uint64 {
	return append([]uint64(nil), j.segments...)
}

// Append writes a new record and returns its sequence number.
func (j *Journal) Append(typ string, payload []byte) (uint64, error) {
	if j.closed {
		return 0, ErrClosed
	}
	seq := j.lastSeq + 1
	buf, err := record.Encode(record.Record{Seq: seq, Type: typ, Payload: payload})
	if err != nil {
		return 0, err
	}
	if len(j.segments) == 0 || (j.tailSize > 0 && j.tailSize+int64(len(buf)) > j.segSize) {
		j.segments = append(j.segments, seq)
		j.tailSize = 0
	}
	p := j.path(segName(j.segments[len(j.segments)-1]))
	f, err := os.OpenFile(p, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return 0, err
	}
	_, werr := f.Write(buf)
	cerr := f.Close()
	if werr != nil {
		return 0, werr
	}
	if cerr != nil {
		return 0, cerr
	}
	j.tailSize += int64(len(buf))
	j.lastSeq = seq
	return seq, nil
}

// Replay calls fn for every record with Seq >= from, in order.
func (j *Journal) Replay(from uint64, fn func(record.Record) error) error {
	if j.closed {
		return ErrClosed
	}
	if from == 0 {
		from = 1
	}
	if from < j.FirstSeq() {
		return ErrCompacted
	}
	for i, first := range j.segments {
		if i+1 < len(j.segments) && j.segments[i+1] <= from {
			continue
		}
		data, err := os.ReadFile(j.path(segName(first)))
		if err != nil {
			return err
		}
		for off := 0; off < len(data); {
			r, n, err := record.Decode(data[off:])
			if err != nil {
				return fmt.Errorf("%w: %v", ErrCorrupt, err)
			}
			off += n
			if r.Seq >= from {
				if err := fn(r); err != nil {
					return err
				}
			}
		}
	}
	return nil
}

// Compact removes every segment, except the last, whose records all have Seq < upTo.
func (j *Journal) Compact(upTo uint64) (int, error) {
	if j.closed {
		return 0, ErrClosed
	}
	removed := 0
	for len(j.segments) > 1 && j.segments[1] <= upTo {
		if err := os.Remove(j.path(segName(j.segments[0]))); err != nil {
			return removed, err
		}
		j.segments = j.segments[1:]
		removed++
	}
	return removed, nil
}

// SaveSnapshot stores data as the state after record seq.
func (j *Journal) SaveSnapshot(seq uint64, data []byte) error {
	if j.closed {
		return ErrClosed
	}
	if seq == 0 || seq > j.lastSeq {
		return ErrBadSnapshot
	}
	buf := make([]byte, 4+len(data))
	binary.BigEndian.PutUint32(buf, crc32.ChecksumIEEE(data))
	copy(buf[4:], data)
	tmp := j.path(snapName(seq) + ".tmp")
	if err := os.WriteFile(tmp, buf, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, j.path(snapName(seq)))
}

// LatestSnapshot returns the valid snapshot with the highest sequence number.
func (j *Journal) LatestSnapshot() (uint64, []byte, error) {
	if j.closed {
		return 0, nil, ErrClosed
	}
	entries, err := os.ReadDir(j.dir)
	if err != nil {
		return 0, nil, err
	}
	var seqs []uint64
	for _, e := range entries {
		if n, ok := parseName(e.Name(), "snapshot-", ".snap"); ok && !e.IsDir() {
			seqs = append(seqs, n)
		}
	}
	sort.Slice(seqs, func(a, b int) bool { return seqs[a] > seqs[b] })
	for _, s := range seqs {
		buf, err := os.ReadFile(j.path(snapName(s)))
		if err != nil || len(buf) < 4 {
			continue
		}
		if crc32.ChecksumIEEE(buf[4:]) == binary.BigEndian.Uint32(buf[:4]) {
			return s, buf[4:], nil
		}
	}
	return 0, nil, ErrNoSnapshot
}

// Close releases the journal. Later calls that touch files return ErrClosed.
func (j *Journal) Close() error {
	if j.closed {
		return ErrClosed
	}
	j.closed = true
	return nil
}
