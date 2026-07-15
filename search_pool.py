import struct, sys

with open(sys.argv[1], 'rb') as f:
    data = f.read()

sp_off = 88
sp_count = struct.unpack_from('<I', data, sp_off)[0]
pool = []
pos = sp_off + 4
for _ in range(sp_count):
    slen = struct.unpack_from('<H', data, pos)[0]
    pos += 2
    s = data[pos:pos+slen].decode('utf-8', errors='replace')
    pool.append(s)
    pos += slen

print(f'Total strings: {len(pool)}')
for pattern in ['исаков', 'проспект', 'проспекту', 'исакова']:
    matches = [s for s in pool if pattern in s.lower()]
    if matches:
        print(f'\n--- Matching "{pattern}" ({len(matches)}) ---')
        for s in matches:
            print(f'  {s!r}')
    else:
        print(f'\n--- Matching "{pattern}": NONE ---')
