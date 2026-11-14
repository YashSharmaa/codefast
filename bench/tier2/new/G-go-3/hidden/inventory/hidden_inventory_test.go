package inventory_test

import (
	"errors"
	"fmt"
	"reflect"
	"testing"

	"example.com/journal/inventory"
	"example.com/journal/journal"
	"example.com/journal/record"
)

func ev(seq uint64, typ, payload string) record.Record {
	return record.Record{Seq: seq, Type: typ, Payload: []byte(payload)}
}

func openJ(t *testing.T, dir string, size int64) *journal.Journal {
	t.Helper()
	j, err := journal.Open(dir, journal.Options{SegmentSize: size})
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	return j
}

func load(t *testing.T, j *journal.Journal) *inventory.Store {
	t.Helper()
	s, err := inventory.Load(j)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	return s
}

func TestApplyAddRemoveAndItems(t *testing.T) {
	inv := inventory.New()
	if items := inv.Items(); items == nil || len(items) != 0 {
		t.Fatalf("empty Items = %#v", items)
	}
	for i, e := range []record.Record{
		ev(1, "add", "pear 4"), ev(2, "add", "apple 10"), ev(3, "remove", "apple 3"),
		ev(4, "add", "Zed-1.x_y 2"), ev(5, "remove", "pear 4"), ev(6, "add", "apple 7"),
	} {
		if err := inv.Apply(e); err != nil {
			t.Fatalf("event %d: %v", i+1, err)
		}
	}
	want := []inventory.Item{{SKU: "Zed-1.x_y", Qty: 2}, {SKU: "apple", Qty: 14}}
	if got := inv.Items(); !reflect.DeepEqual(got, want) {
		t.Fatalf("Items = %+v, want %+v", got, want)
	}
	if inv.Stock("pear") != 0 || inv.Stock("missing") != 0 || inv.Stock("apple") != 14 {
		t.Fatalf("Stock values wrong")
	}
}

func TestApplyRejectsBadEventsWithoutChange(t *testing.T) {
	inv := inventory.New()
	inv.Apply(ev(1, "add", "apple 5"))
	for _, e := range []record.Record{
		ev(2, "sell", "apple 1"),
		ev(2, "add", "apple"),
		ev(2, "add", "apple 0"),
		ev(2, "add", "apple -1"),
		ev(2, "add", "apple +1"),
		ev(2, "add", "apple  1"),
		ev(2, "add", "apple 1 "),
		ev(2, "add", " apple 1"),
		ev(2, "add", "app/le 1"),
		ev(2, "add", "apple 1.5"),
		ev(2, "add", ""),
	} {
		if err := inv.Apply(e); !errors.Is(err, inventory.ErrMalformed) {
			t.Fatalf("%s %q: got %v, want ErrMalformed", e.Type, e.Payload, err)
		}
	}
	if err := inv.Apply(ev(3, "remove", "apple 6")); !errors.Is(err, inventory.ErrInsufficient) {
		t.Fatalf("over-remove: %v", err)
	}
	if err := inv.Apply(ev(3, "remove", "kiwi 1")); !errors.Is(err, inventory.ErrInsufficient) {
		t.Fatalf("remove unknown sku: %v", err)
	}
	if got := inv.Items(); !reflect.DeepEqual(got, []inventory.Item{{SKU: "apple", Qty: 5}}) {
		t.Fatalf("failed events changed state: %+v", got)
	}
}

func TestEncodeAndDecode(t *testing.T) {
	inv := inventory.New()
	inv.Apply(ev(1, "add", "b 2"))
	inv.Apply(ev(2, "add", "a 10"))
	inv.Apply(ev(3, "add", "c 1"))
	inv.Apply(ev(4, "remove", "c 1"))
	if got := string(inv.Encode()); got != "a 10\nb 2\n" {
		t.Fatalf("Encode = %q", got)
	}
	if got := string(inventory.New().Encode()); got != "" {
		t.Fatalf("empty Encode = %q", got)
	}
	back, err := inventory.Decode([]byte("b 2\na 10\n"))
	if err != nil || !reflect.DeepEqual(back.Items(), inv.Items()) {
		t.Fatalf("Decode = %+v, %v", back, err)
	}
	if empty, err := inventory.Decode(nil); err != nil || len(empty.Items()) != 0 {
		t.Fatalf("Decode(empty) = %v", err)
	}
	for _, bad := range []string{"a 10", "a 10\na 2\n", "a\n", "a 0\n", "\n", "a 1\n\n", "a b 1\n"} {
		if _, err := inventory.Decode([]byte(bad)); !errors.Is(err, inventory.ErrMalformed) {
			t.Fatalf("Decode(%q) = %v, want ErrMalformed", bad, err)
		}
	}
}

func TestStorePersistsThroughJournal(t *testing.T) {
	dir := t.TempDir()
	j := openJ(t, dir, 0)
	s := load(t, j)
	if seq, err := s.Add("widget", 7); err != nil || seq != 1 {
		t.Fatalf("Add = %d, %v", seq, err)
	}
	if seq, err := s.Remove("widget", 2); err != nil || seq != 2 {
		t.Fatalf("Remove = %d, %v", seq, err)
	}
	s.Add("gadget", 1)
	if s.Inventory().Stock("widget") != 5 {
		t.Fatalf("Stock = %d", s.Inventory().Stock("widget"))
	}
	var got []string
	j.Replay(1, func(r record.Record) error {
		got = append(got, fmt.Sprintf("%d %s %s", r.Seq, r.Type, r.Payload))
		return nil
	})
	if !reflect.DeepEqual(got, []string{"1 add widget 7", "2 remove widget 2", "3 add gadget 1"}) {
		t.Fatalf("journal = %q", got)
	}
	j.Close()
	s2 := load(t, openJ(t, dir, 0))
	want := []inventory.Item{{SKU: "gadget", Qty: 1}, {SKU: "widget", Qty: 5}}
	if !reflect.DeepEqual(s2.Inventory().Items(), want) {
		t.Fatalf("after reload Items = %+v", s2.Inventory().Items())
	}
}

func TestStoreRefusalsAppendNothing(t *testing.T) {
	j := openJ(t, t.TempDir(), 0)
	s := load(t, j)
	s.Add("bolt", 3)
	if _, err := s.Remove("bolt", 4); err != inventory.ErrInsufficient {
		t.Fatalf("Remove too many = %v, want ErrInsufficient itself", err)
	}
	if _, err := s.Add("bad sku", 1); err != inventory.ErrMalformed {
		t.Fatalf("bad sku = %v", err)
	}
	if _, err := s.Add("bolt", 0); err != inventory.ErrMalformed {
		t.Fatalf("qty 0 = %v", err)
	}
	if _, err := s.Remove("bolt", -2); err != inventory.ErrMalformed {
		t.Fatalf("negative qty = %v", err)
	}
	if _, err := s.Add("", 1); err != inventory.ErrMalformed {
		t.Fatalf("empty sku = %v", err)
	}
	if j.LastSeq() != 1 || s.Inventory().Stock("bolt") != 3 {
		t.Fatalf("refused operations must not append: LastSeq=%d", j.LastSeq())
	}
}

func TestLoadUsesLatestSnapshotThenReplays(t *testing.T) {
	dir := t.TempDir()
	j := openJ(t, dir, 60)
	s := load(t, j)
	for i := 0; i < 6; i++ {
		s.Add("nut", 1)
	}
	seq, err := s.Snapshot()
	if err != nil || seq != 6 {
		t.Fatalf("Snapshot = %d, %v", seq, err)
	}
	s.Add("nut", 10)
	s.Remove("nut", 4)
	if n, _ := j.Compact(7); n == 0 {
		t.Fatalf("expected old segments to be compacted, segments %v", j.Segments())
	}
	j.Close()
	j = openJ(t, dir, 60)
	s = load(t, j)
	if got := s.Inventory().Stock("nut"); got != 12 {
		t.Fatalf("Stock = %d, want 12 (snapshot 6, then +10 -4)", got)
	}
	if _, data, _ := j.LatestSnapshot(); string(data) != "nut 6\n" {
		t.Fatalf("snapshot data = %q", data)
	}
}

func TestLoadErrors(t *testing.T) {
	dir := t.TempDir()
	j := openJ(t, dir, 0)
	j.Append("add", []byte("x 1"))
	j.Append("remove", []byte("x 5"))
	if _, err := inventory.Load(j); !errors.Is(err, inventory.ErrInsufficient) {
		t.Fatalf("over-remove in journal: %v", err)
	}
	j2 := openJ(t, t.TempDir(), 0)
	j2.Append("restock", []byte("x 1"))
	if _, err := inventory.Load(j2); !errors.Is(err, inventory.ErrMalformed) {
		t.Fatalf("unknown type in journal: %v", err)
	}
	j3 := openJ(t, t.TempDir(), 25)
	for i := 0; i < 4; i++ {
		j3.Append("add", []byte("y 1"))
	}
	j3.Compact(3)
	if _, err := inventory.Load(j3); !errors.Is(err, journal.ErrCompacted) {
		t.Fatalf("compacted without snapshot: %v", err)
	}
	j4 := openJ(t, t.TempDir(), 0)
	j4.Append("add", []byte("z 1"))
	j4.SaveSnapshot(1, []byte("not a snapshot"))
	if _, err := inventory.Load(j4); !errors.Is(err, inventory.ErrMalformed) {
		t.Fatalf("bad snapshot data: %v", err)
	}
}

func TestSnapshotOnEmptyJournal(t *testing.T) {
	s := load(t, openJ(t, t.TempDir(), 0))
	if _, err := s.Snapshot(); !errors.Is(err, journal.ErrBadSnapshot) {
		t.Fatalf("Snapshot on empty journal = %v", err)
	}
}
