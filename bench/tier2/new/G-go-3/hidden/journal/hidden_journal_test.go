package journal_test

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"example.com/journal/journal"
	"example.com/journal/record"
)

func seg(dir string, first uint64) string {
	return filepath.Join(dir, fmt.Sprintf("%020d.log", first))
}

func enc(t *testing.T, seq uint64, typ, payload string) []byte {
	t.Helper()
	b, err := record.Encode(record.Record{Seq: seq, Type: typ, Payload: []byte(payload)})
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func open(t *testing.T, dir string, size int64) *journal.Journal {
	t.Helper()
	j, err := journal.Open(dir, journal.Options{SegmentSize: size})
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	return j
}

func appendN(t *testing.T, j *journal.Journal, payloads ...string) {
	t.Helper()
	for _, p := range payloads {
		if _, err := j.Append("ev", []byte(p)); err != nil {
			t.Fatalf("Append: %v", err)
		}
	}
}

func collect(t *testing.T, j *journal.Journal, from uint64) []string {
	t.Helper()
	var out []string
	err := j.Replay(from, func(r record.Record) error {
		out = append(out, fmt.Sprintf("%d:%s:%s", r.Seq, r.Type, r.Payload))
		return nil
	})
	if err != nil {
		t.Fatalf("Replay: %v", err)
	}
	return out
}

func fileSize(t *testing.T, p string) int64 {
	t.Helper()
	st, err := os.Stat(p)
	if err != nil {
		t.Fatal(err)
	}
	return st.Size()
}

// five records of 25 bytes each ("ev" + 5-byte payload), segment size 50: two per segment.
func rotated(t *testing.T, dir string) *journal.Journal {
	t.Helper()
	j := open(t, dir, 50)
	appendN(t, j, "aaaaa", "bbbbb", "ccccc", "ddddd", "eeeee")
	return j
}

func TestOpenEmptyDirectory(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "nested", "journal")
	j := open(t, dir, 0)
	if j.LastSeq() != 0 || j.FirstSeq() != 1 || len(j.Segments()) != 0 {
		t.Fatalf("empty journal: last=%d first=%d segs=%v", j.LastSeq(), j.FirstSeq(), j.Segments())
	}
	os.WriteFile(filepath.Join(dir, "notes.txt"), []byte("hello"), 0o644)
	os.WriteFile(filepath.Join(dir, "1.log"), []byte("garbage"), 0o644)
	j.Close()
	j = open(t, dir, 0)
	if j.LastSeq() != 0 || len(j.Segments()) != 0 {
		t.Fatalf("other files must be ignored: segs=%v", j.Segments())
	}
}

func TestAppendNumbersAndReopenContinues(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	for want := uint64(1); want <= 3; want++ {
		seq, err := j.Append("add", []byte(fmt.Sprintf("item %d", want)))
		if err != nil || seq != want {
			t.Fatalf("Append = %d, %v; want %d", seq, err, want)
		}
	}
	if err := j.Close(); err != nil {
		t.Fatal(err)
	}
	j = open(t, dir, 0)
	if j.LastSeq() != 3 {
		t.Fatalf("LastSeq after reopen = %d", j.LastSeq())
	}
	if seq, _ := j.Append("add", []byte("item 4")); seq != 4 {
		t.Fatalf("Append after reopen = %d", seq)
	}
	if !reflect.DeepEqual(j.Segments(), []uint64{1}) {
		t.Fatalf("Segments = %v", j.Segments())
	}
}

func TestSegmentFileHoldsEncodedRecords(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	j.Append("add", []byte("apple 5"))
	j.Append("remove", []byte("apple 2"))
	j.Append("note", nil)
	got, err := os.ReadFile(seg(dir, 1))
	if err != nil {
		t.Fatal(err)
	}
	want := append(append(enc(t, 1, "add", "apple 5"), enc(t, 2, "remove", "apple 2")...), enc(t, 3, "note", "")...)
	if !bytes.Equal(got, want) {
		t.Fatalf("segment bytes differ:\n% x\n% x", got, want)
	}
}

func TestRotationBySegmentSize(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	if !reflect.DeepEqual(j.Segments(), []uint64{1, 3, 5}) {
		t.Fatalf("Segments = %v", j.Segments())
	}
	if fileSize(t, seg(dir, 1)) != 50 || fileSize(t, seg(dir, 3)) != 50 || fileSize(t, seg(dir, 5)) != 25 {
		t.Fatalf("unexpected segment sizes")
	}
	j.Close()
	j = open(t, dir, 50)
	appendN(t, j, "fffff", "ggggg")
	if !reflect.DeepEqual(j.Segments(), []uint64{1, 3, 5, 7}) {
		t.Fatalf("after reopen Segments = %v", j.Segments())
	}
}

func TestOversizedRecordSitsAlone(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 50)
	appendN(t, j, "x", "this payload is far longer than fifty bytes in total, really", "y", "z")
	if !reflect.DeepEqual(j.Segments(), []uint64{1, 2, 3}) {
		t.Fatalf("Segments = %v", j.Segments())
	}
	if got := collect(t, j, 0); len(got) != 4 || got[3] != "4:ev:z" {
		t.Fatalf("replay = %v", got)
	}
}

func TestReplayFromAndCallbackError(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	if got := collect(t, j, 4); !reflect.DeepEqual(got, []string{"4:ev:ddddd", "5:ev:eeeee"}) {
		t.Fatalf("Replay(4) = %v", got)
	}
	if got := collect(t, j, 0); len(got) != 5 || got[0] != "1:ev:aaaaa" {
		t.Fatalf("Replay(0) = %v", got)
	}
	if got := collect(t, j, 9); len(got) != 0 {
		t.Fatalf("Replay(9) = %v", got)
	}
	stop := errors.New("stop here")
	calls := 0
	err := j.Replay(2, func(r record.Record) error {
		calls++
		if r.Seq == 3 {
			return stop
		}
		return nil
	})
	if err != stop || calls != 2 {
		t.Fatalf("err=%v calls=%d", err, calls)
	}
}

func TestTornTailIsTruncated(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "one", "two", "three")
	j.Close()
	p := seg(dir, 1)
	full := fileSize(t, p)
	if err := os.Truncate(p, full-4); err != nil {
		t.Fatal(err)
	}
	j = open(t, dir, 0)
	if j.LastSeq() != 2 || fileSize(t, p) != 2*(18+2+3) {
		t.Fatalf("LastSeq=%d size=%d", j.LastSeq(), fileSize(t, p))
	}
	if seq, _ := j.Append("ev", []byte("again")); seq != 3 {
		t.Fatalf("next seq = %d", seq)
	}
	if got := collect(t, j, 1); !reflect.DeepEqual(got, []string{"1:ev:one", "2:ev:two", "3:ev:again"}) {
		t.Fatalf("replay = %v", got)
	}
}

func TestPartialHeaderAtEndIsTruncated(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "one", "two")
	j.Close()
	p := seg(dir, 1)
	before := fileSize(t, p)
	f, _ := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0o644)
	f.Write([]byte{0, 0, 0, 30, 1})
	f.Close()
	j = open(t, dir, 0)
	if j.LastSeq() != 2 || fileSize(t, p) != before {
		t.Fatalf("LastSeq=%d size=%d want %d", j.LastSeq(), fileSize(t, p), before)
	}
}

func TestBadFinalRecordIsDropped(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "one", "two", "three")
	j.Close()
	p := seg(dir, 1)
	data, _ := os.ReadFile(p)
	data[len(data)-2] ^= 0xff
	os.WriteFile(p, data, 0o644)
	j = open(t, dir, 0)
	if j.LastSeq() != 2 || fileSize(t, p) != 46 {
		t.Fatalf("LastSeq=%d size=%d", j.LastSeq(), fileSize(t, p))
	}
}

func TestBadRecordFollowedByMoreIsCorrupt(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "one", "two", "three")
	j.Close()
	p := seg(dir, 1)
	data, _ := os.ReadFile(p)
	data[23+20] ^= 0x01
	os.WriteFile(p, data, 0o644)
	if _, err := journal.Open(dir, journal.Options{}); !errors.Is(err, journal.ErrCorrupt) {
		t.Fatalf("got %v, want ErrCorrupt", err)
	}
	if got := fileSize(t, p); got != int64(len(data)) {
		t.Fatalf("a corrupt journal must not be truncated, size %d", got)
	}
}

func TestDamageInEarlierSegmentIsCorrupt(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	j.Close()
	if err := os.Truncate(seg(dir, 3), 45); err != nil {
		t.Fatal(err)
	}
	if _, err := journal.Open(dir, journal.Options{SegmentSize: 50}); !errors.Is(err, journal.ErrCorrupt) {
		t.Fatalf("torn record in a non-last segment: got %v", err)
	}
	if got := fileSize(t, seg(dir, 3)); got != 45 {
		t.Fatalf("Open must not change files when it reports ErrCorrupt, size %d", got)
	}
}

func TestSequenceBreaksAreCorrupt(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	j.Close()
	os.Remove(seg(dir, 3))
	if _, err := journal.Open(dir, journal.Options{}); !errors.Is(err, journal.ErrCorrupt) {
		t.Fatalf("missing middle segment: got %v", err)
	}
	dir2 := t.TempDir()
	os.WriteFile(seg(dir2, 1), append(enc(t, 1, "ev", "a"), enc(t, 3, "ev", "b")...), 0o644)
	if _, err := journal.Open(dir2, journal.Options{}); !errors.Is(err, journal.ErrCorrupt) {
		t.Fatalf("record numbering gap: got %v", err)
	}
	dir3 := t.TempDir()
	os.WriteFile(seg(dir3, 5), enc(t, 4, "ev", "a"), 0o644)
	if _, err := journal.Open(dir3, journal.Options{}); !errors.Is(err, journal.ErrCorrupt) {
		t.Fatalf("first record must match the segment name: got %v", err)
	}
}

func TestEmptyLastSegmentAfterTornWrite(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	j.Close()
	os.Truncate(seg(dir, 5), 10)
	j = open(t, dir, 50)
	if j.LastSeq() != 4 || !reflect.DeepEqual(j.Segments(), []uint64{1, 3, 5}) || fileSize(t, seg(dir, 5)) != 0 {
		t.Fatalf("LastSeq=%d Segments=%v", j.LastSeq(), j.Segments())
	}
	if seq, _ := j.Append("ev", []byte("again")); seq != 5 || fileSize(t, seg(dir, 5)) != 25 {
		t.Fatalf("seq=%d", seq)
	}
	dir2 := t.TempDir()
	os.WriteFile(seg(dir2, 8), nil, 0o644)
	j2 := open(t, dir2, 0)
	if j2.LastSeq() != 7 || j2.FirstSeq() != 8 {
		t.Fatalf("lone empty segment 8: LastSeq=%d FirstSeq=%d", j2.LastSeq(), j2.FirstSeq())
	}
}

func TestCompactRemovesOnlyWholeOldSegments(t *testing.T) {
	dir := t.TempDir()
	j := rotated(t, dir)
	appendN(t, j, "fffff", "ggggg")
	if n, err := j.Compact(4); err != nil || n != 1 {
		t.Fatalf("Compact(4) = %d, %v", n, err)
	}
	if !reflect.DeepEqual(j.Segments(), []uint64{3, 5, 7}) || j.FirstSeq() != 3 {
		t.Fatalf("Segments = %v", j.Segments())
	}
	if _, err := os.Stat(seg(dir, 1)); !os.IsNotExist(err) {
		t.Fatalf("segment 1 must be deleted")
	}
	if n, _ := j.Compact(5); n != 1 {
		t.Fatalf("Compact(5) = %d", n)
	}
	if n, _ := j.Compact(1000); n != 1 || !reflect.DeepEqual(j.Segments(), []uint64{7}) {
		t.Fatalf("the last segment must stay: n=%d %v", n, j.Segments())
	}
	if err := j.Replay(6, func(record.Record) error { return nil }); !errors.Is(err, journal.ErrCompacted) {
		t.Fatalf("Replay(6) = %v", err)
	}
	if got := collect(t, j, 7); !reflect.DeepEqual(got, []string{"7:ev:ggggg"}) {
		t.Fatalf("Replay(7) = %v", got)
	}
	j.Close()
	j = open(t, dir, 50)
	if j.FirstSeq() != 7 || j.LastSeq() != 7 {
		t.Fatalf("reopen: first=%d last=%d", j.FirstSeq(), j.LastSeq())
	}
}

func TestSnapshotsFilesAndLatest(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	if _, _, err := j.LatestSnapshot(); !errors.Is(err, journal.ErrNoSnapshot) {
		t.Fatalf("no snapshot yet: %v", err)
	}
	if err := j.SaveSnapshot(1, []byte("x")); !errors.Is(err, journal.ErrBadSnapshot) {
		t.Fatalf("snapshot on empty journal: %v", err)
	}
	appendN(t, j, "a", "b", "c", "d")
	if err := j.SaveSnapshot(0, nil); !errors.Is(err, journal.ErrBadSnapshot) {
		t.Fatalf("seq 0: %v", err)
	}
	if err := j.SaveSnapshot(5, nil); !errors.Is(err, journal.ErrBadSnapshot) {
		t.Fatalf("seq beyond LastSeq: %v", err)
	}
	if err := j.SaveSnapshot(4, []byte("state four")); err != nil {
		t.Fatal(err)
	}
	if err := j.SaveSnapshot(2, []byte("state two")); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(filepath.Join(dir, "snapshot-00000000000000000004.snap"))
	if err != nil {
		t.Fatal(err)
	}
	if binary.BigEndian.Uint32(raw[:4]) != crc32.ChecksumIEEE([]byte("state four")) || string(raw[4:]) != "state four" {
		t.Fatalf("snapshot file = % x", raw)
	}
	seq, data, err := j.LatestSnapshot()
	if err != nil || seq != 4 || string(data) != "state four" {
		t.Fatalf("LatestSnapshot = %d %q %v", seq, data, err)
	}
	j.SaveSnapshot(4, []byte("state four again"))
	if _, data, _ = j.LatestSnapshot(); string(data) != "state four again" {
		t.Fatalf("saving the same seq must replace it, got %q", data)
	}
}

func TestDamagedSnapshotsAreSkipped(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "a", "b", "c")
	j.SaveSnapshot(1, []byte("one"))
	j.SaveSnapshot(3, []byte("three"))
	p3 := filepath.Join(dir, "snapshot-00000000000000000003.snap")
	raw, _ := os.ReadFile(p3)
	raw[len(raw)-1] ^= 0x01
	os.WriteFile(p3, raw, 0o644)
	os.WriteFile(filepath.Join(dir, "snapshot-00000000000000000002.snap"), []byte{1, 2}, 0o644)
	os.WriteFile(filepath.Join(dir, "snapshot-9.snap"), []byte("junk"), 0o644)
	seq, data, err := j.LatestSnapshot()
	if err != nil || seq != 1 || string(data) != "one" {
		t.Fatalf("LatestSnapshot = %d %q %v", seq, data, err)
	}
}

func TestClosedJournal(t *testing.T) {
	dir := t.TempDir()
	j := open(t, dir, 0)
	appendN(t, j, "a", "b")
	if err := j.Close(); err != nil {
		t.Fatal(err)
	}
	if _, err := j.Append("ev", nil); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("Append: %v", err)
	}
	if err := j.Replay(1, func(record.Record) error { return nil }); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("Replay: %v", err)
	}
	if _, err := j.Compact(5); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("Compact: %v", err)
	}
	if err := j.SaveSnapshot(1, nil); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("SaveSnapshot: %v", err)
	}
	if _, _, err := j.LatestSnapshot(); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("LatestSnapshot: %v", err)
	}
	if err := j.Close(); !errors.Is(err, journal.ErrClosed) {
		t.Fatalf("second Close: %v", err)
	}
	if j.LastSeq() != 2 {
		t.Fatalf("LastSeq after Close = %d", j.LastSeq())
	}
	if _, err := open(t, dir, 0).Append("", nil); !errors.Is(err, record.ErrType) {
		t.Fatalf("empty type: %v", err)
	}
}
