package record_test

import (
	"bytes"
	"encoding/binary"
	"errors"
	"hash/crc32"
	"strings"
	"testing"

	"example.com/journal/record"
)

func manual(seq uint64, typ string, payload []byte) []byte {
	body := make([]byte, 10)
	binary.BigEndian.PutUint64(body[0:8], seq)
	binary.BigEndian.PutUint16(body[8:10], uint16(len(typ)))
	body = append(body, typ...)
	body = append(body, payload...)
	head := make([]byte, 8)
	binary.BigEndian.PutUint32(head[0:4], uint32(len(body)))
	binary.BigEndian.PutUint32(head[4:8], crc32.ChecksumIEEE(body))
	return append(head, body...)
}

func TestEncodeLayoutExact(t *testing.T) {
	got, err := record.Encode(record.Record{Seq: 0x0102030405060708, Type: "add", Payload: []byte("apple 3")})
	if err != nil {
		t.Fatal(err)
	}
	want := manual(0x0102030405060708, "add", []byte("apple 3"))
	if !bytes.Equal(got, want) {
		t.Fatalf("Encode = % x\nwant     % x", got, want)
	}
	if len(got) != 8+10+3+7 || binary.BigEndian.Uint32(got[0:4]) != 20 {
		t.Fatalf("length field = %d, size %d", binary.BigEndian.Uint32(got[0:4]), len(got))
	}
}

func TestDecodeRoundTripIgnoresTrailingBytes(t *testing.T) {
	enc := manual(42, "remove", []byte("pear 10"))
	buf := append(append([]byte{}, enc...), 0xde, 0xad, 0xbe, 0xef)
	r, n, err := record.Decode(buf)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(enc) || r.Seq != 42 || r.Type != "remove" || string(r.Payload) != "pear 10" {
		t.Fatalf("got %+v n=%d", r, n)
	}
	empty := manual(7, "x", nil)
	r, n, err = record.Decode(empty)
	if err != nil || n != 19 || r.Payload == nil || len(r.Payload) != 0 || r.Type != "x" {
		t.Fatalf("empty payload: %+v n=%d err=%v", r, n, err)
	}
}

func TestDecodeShort(t *testing.T) {
	enc := manual(1, "add", []byte("a 1"))
	for _, cut := range []int{0, 3, 7, 8, 12, len(enc) - 1} {
		if _, _, err := record.Decode(enc[:cut]); !errors.Is(err, record.ErrShort) {
			t.Fatalf("cut at %d: got %v, want ErrShort", cut, err)
		}
	}
}

func TestDecodeChecksumMismatch(t *testing.T) {
	enc := manual(1, "add", []byte("a 1"))
	for _, pos := range []int{4, 9, 17, len(enc) - 1} {
		bad := append([]byte{}, enc...)
		bad[pos] ^= 0x40
		if _, _, err := record.Decode(bad); !errors.Is(err, record.ErrChecksum) {
			t.Fatalf("flip at %d: got %v, want ErrChecksum", pos, err)
		}
	}
}

func TestDecodeInvalid(t *testing.T) {
	small := make([]byte, 8+9)
	binary.BigEndian.PutUint32(small[0:4], 9)
	binary.BigEndian.PutUint32(small[4:8], crc32.ChecksumIEEE(small[8:]))
	if _, _, err := record.Decode(small); !errors.Is(err, record.ErrInvalid) {
		t.Fatalf("L=9: got %v", err)
	}
	if _, _, err := record.Decode(small[:8]); !errors.Is(err, record.ErrInvalid) {
		t.Fatalf("L=9 is checked before the length of the input: got %v", err)
	}
	noType := manual(3, "", []byte("abc"))
	if _, _, err := record.Decode(noType); !errors.Is(err, record.ErrInvalid) {
		t.Fatalf("type length 0: got %v", err)
	}
	long := manual(3, "add", []byte("xy"))
	binary.BigEndian.PutUint16(long[16:18], 6)
	binary.BigEndian.PutUint32(long[4:8], crc32.ChecksumIEEE(long[8:]))
	if _, _, err := record.Decode(long); !errors.Is(err, record.ErrInvalid) {
		t.Fatalf("type length beyond body: got %v", err)
	}
}

func TestEncodeRejectsBadType(t *testing.T) {
	if _, err := record.Encode(record.Record{Seq: 1, Type: ""}); !errors.Is(err, record.ErrType) {
		t.Fatalf("empty type: %v", err)
	}
	if _, err := record.Encode(record.Record{Seq: 1, Type: strings.Repeat("t", 65536)}); !errors.Is(err, record.ErrType) {
		t.Fatalf("long type: %v", err)
	}
	enc, err := record.Encode(record.Record{Seq: 1, Type: strings.Repeat("t", 65535)})
	if err != nil || len(enc) != 8+10+65535 {
		t.Fatalf("65535-byte type must work: len %d err %v", len(enc), err)
	}
}
