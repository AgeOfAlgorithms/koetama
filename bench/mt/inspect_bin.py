"""Reads a Marian binary model (Mozilla's model.*.intgemm.alphas.bin): its tensors (name, type, shape, bytes) and its
embedded config (special:model.yml). Marian's format (src/common/binary.cpp): u64 version, u64 count, count headers of
4 x u64 (name length, type, shape length, data length), the names, the shapes (i32), u64 padding, the data."""
import struct
import sys

TYPES = {0x0100: 'int8', 0x0200: 'int16', 0x0400: 'int32', 0x0800: 'int64', 0x1100: 'uint8', 0x1200: 'uint16',
         0x1400: 'uint32', 0x1800: 'uint64', 0x2200: 'float16', 0x2400: 'float32', 0x2800: 'float64'}


def read(path):
    b = open(path, 'rb').read()
    ver, n = struct.unpack_from('<QQ', b, 0)
    off = 16
    heads = []
    for _ in range(n):
        heads.append(struct.unpack_from('<QQQQ', b, off))
        off += 32
    names = []
    for nl, _, _, _ in heads:
        names.append(b[off:off + nl - 1].decode())
        off += nl
    shapes = []
    for _, _, sl, _ in heads:
        shapes.append(struct.unpack_from('<' + 'i' * sl, b, off))
        off += 4 * sl
    (pad,) = struct.unpack_from('<Q', b, off)
    off += 8 + pad
    items = []
    for (nl, t, sl, dl), name, shape in zip(heads, names, shapes):
        items.append((name, t, shape, dl, off))
        off += dl
    return ver, items, b


if __name__ == '__main__':
    ver, items, b = read(sys.argv[1])
    print('version', ver, 'tensors', len(items), 'end', items[-1][4] + items[-1][3], 'file', len(b))
    for name, t, shape, dl, off in items:
        tn = TYPES.get(t & 0xFFFF, hex(t))
        if name == 'special:model.yml':
            print('---- special:model.yml ----')
            print(b[off:off + dl].decode(errors='replace').rstrip('\x00'))
            continue
        print(f'{name:<45} {hex(t):>8} {tn:<8} {str(shape):<18} {dl}')
