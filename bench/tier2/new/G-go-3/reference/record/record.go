// Package record encodes and decodes journal records.
package record

import (
	"encoding/binary"
	"errors"
	"hash/crc32"
)

// Record is one journal entry.
type Record struct {
	Seq     uint64
	Type    string
	Payload []byte
}

var (
	ErrShort    = errors.New("record: short")
	ErrInvalid  = errors.New("record: invalid")
	ErrChecksum = errors.New("record: checksum mismatch")
	ErrType     = errors.New("record: bad type")
)

// HeaderSize is the size of the length and checksum fields.
const HeaderSize = 8

// minBody is the size of the sequence number and type length fields.
const minBody = 10

// Encode returns the binary form of r.
func Encode(r Record) ([]byte, error) {
	if len(r.Type) == 0 || len(r.Type) > 0xFFFF {
		return nil, ErrType
	}
	bodyLen := minBody + len(r.Type) + len(r.Payload)
	buf := make([]byte, HeaderSize+bodyLen)
	body := buf[HeaderSize:]
	binary.BigEndian.PutUint64(body[0:8], r.Seq)
	binary.BigEndian.PutUint16(body[8:10], uint16(len(r.Type)))
	copy(body[10:], r.Type)
	copy(body[10+len(r.Type):], r.Payload)
	binary.BigEndian.PutUint32(buf[0:4], uint32(bodyLen))
	binary.BigEndian.PutUint32(buf[4:8], crc32.ChecksumIEEE(body))
	return buf, nil
}

// Decode reads the record at the start of b and returns it with its size.
func Decode(b []byte) (Record, int, error) {
	if len(b) < HeaderSize {
		return Record{}, 0, ErrShort
	}
	bodyLen := int64(binary.BigEndian.Uint32(b[0:4]))
	if bodyLen < minBody {
		return Record{}, 0, ErrInvalid
	}
	if int64(len(b)) < HeaderSize+bodyLen {
		return Record{}, 0, ErrShort
	}
	body := b[HeaderSize : HeaderSize+bodyLen]
	if crc32.ChecksumIEEE(body) != binary.BigEndian.Uint32(b[4:8]) {
		return Record{}, 0, ErrChecksum
	}
	typeLen := int64(binary.BigEndian.Uint16(body[8:10]))
	if typeLen == 0 || typeLen > bodyLen-minBody {
		return Record{}, 0, ErrInvalid
	}
	r := Record{
		Seq:     binary.BigEndian.Uint64(body[0:8]),
		Type:    string(body[10 : 10+typeLen]),
		Payload: append([]byte{}, body[10+typeLen:]...),
	}
	return r, int(HeaderSize + bodyLen), nil
}
