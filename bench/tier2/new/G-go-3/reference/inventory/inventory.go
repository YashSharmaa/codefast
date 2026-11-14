// Package inventory is a stock-keeping state machine rebuilt from a journal.
package inventory

import (
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"

	"example.com/journal/journal"
	"example.com/journal/record"
)

var (
	ErrMalformed    = errors.New("inventory: malformed")
	ErrInsufficient = errors.New("inventory: insufficient stock")
)

// Item is one stocked SKU.
type Item struct {
	SKU string
	Qty int
}

// Inventory maps SKUs to quantities.
type Inventory struct {
	stock map[string]int
}

// New returns an empty inventory.
func New() *Inventory { return &Inventory{stock: map[string]int{}} }

func validSKU(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		ok := r == '-' || r == '_' || r == '.' || (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9')
		if !ok {
			return false
		}
	}
	return true
}

// parsePair reads "SKU QTY" with exactly one space and QTY a positive integer.
func parsePair(s string) (string, int, bool) {
	sku, q, found := strings.Cut(s, " ")
	if !found || !validSKU(sku) || q == "" || strings.Trim(q, "0123456789") != "" {
		return "", 0, false
	}
	n, err := strconv.Atoi(q)
	if err != nil || n <= 0 {
		return "", 0, false
	}
	return sku, n, true
}

// Apply applies one journal record.
func (inv *Inventory) Apply(r record.Record) error {
	sku, qty, ok := parsePair(string(r.Payload))
	if !ok {
		return fmt.Errorf("%w: record %d", ErrMalformed, r.Seq)
	}
	switch r.Type {
	case "add":
		inv.stock[sku] += qty
	case "remove":
		if inv.stock[sku] < qty {
			return fmt.Errorf("%w: record %d", ErrInsufficient, r.Seq)
		}
		inv.stock[sku] -= qty
		if inv.stock[sku] == 0 {
			delete(inv.stock, sku)
		}
	default:
		return fmt.Errorf("%w: record %d type %q", ErrMalformed, r.Seq, r.Type)
	}
	return nil
}

// Stock returns the quantity of sku.
func (inv *Inventory) Stock(sku string) int { return inv.stock[sku] }

// Items returns the stocked SKUs sorted by name.
func (inv *Inventory) Items() []Item {
	out := []Item{}
	for s, q := range inv.stock {
		out = append(out, Item{s, q})
	}
	sort.Slice(out, func(a, b int) bool { return out[a].SKU < out[b].SKU })
	return out
}

// Encode returns the snapshot form of the inventory.
func (inv *Inventory) Encode() []byte {
	var b strings.Builder
	for _, it := range inv.Items() {
		fmt.Fprintf(&b, "%s %d\n", it.SKU, it.Qty)
	}
	return []byte(b.String())
}

// Decode parses the snapshot form produced by Encode.
func Decode(data []byte) (*Inventory, error) {
	inv := New()
	s := string(data)
	if s == "" {
		return inv, nil
	}
	if !strings.HasSuffix(s, "\n") {
		return nil, ErrMalformed
	}
	for _, line := range strings.Split(strings.TrimSuffix(s, "\n"), "\n") {
		sku, qty, ok := parsePair(line)
		if !ok || inv.stock[sku] != 0 {
			return nil, ErrMalformed
		}
		inv.stock[sku] = qty
	}
	return inv, nil
}

// Store keeps an inventory in step with a journal.
type Store struct {
	j   *journal.Journal
	inv *Inventory
}

// Load rebuilds the inventory from the latest snapshot and the records after it.
func Load(j *journal.Journal) (*Store, error) {
	inv := New()
	from := uint64(1)
	seq, data, err := j.LatestSnapshot()
	switch {
	case err == nil:
		if inv, err = Decode(data); err != nil {
			return nil, err
		}
		from = seq + 1
	case !errors.Is(err, journal.ErrNoSnapshot):
		return nil, err
	}
	if err := j.Replay(from, inv.Apply); err != nil {
		return nil, err
	}
	return &Store{j: j, inv: inv}, nil
}

// Inventory returns the current state.
func (s *Store) Inventory() *Inventory { return s.inv }

func (s *Store) record(typ, sku string, qty int) (uint64, error) {
	payload := []byte(fmt.Sprintf("%s %d", sku, qty))
	seq, err := s.j.Append(typ, payload)
	if err != nil {
		return 0, err
	}
	return seq, s.inv.Apply(record.Record{Seq: seq, Type: typ, Payload: payload})
}

// Add records an addition of qty units of sku.
func (s *Store) Add(sku string, qty int) (uint64, error) {
	if !validSKU(sku) || qty <= 0 {
		return 0, ErrMalformed
	}
	return s.record("add", sku, qty)
}

// Remove records a removal, refusing it when stock is insufficient.
func (s *Store) Remove(sku string, qty int) (uint64, error) {
	if !validSKU(sku) || qty <= 0 {
		return 0, ErrMalformed
	}
	if s.inv.Stock(sku) < qty {
		return 0, ErrInsufficient
	}
	return s.record("remove", sku, qty)
}

// Snapshot saves the current state at the journal's last sequence number.
func (s *Store) Snapshot() (uint64, error) {
	seq := s.j.LastSeq()
	if err := s.j.SaveSnapshot(seq, s.inv.Encode()); err != nil {
		return 0, err
	}
	return seq, nil
}
