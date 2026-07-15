import struct, sys

path = sys.argv[1]
search = sys.argv[2].lower() if len(sys.argv) > 2 else 'калининград'

with open(path, 'rb') as f:
    data = f.read()

# Header
magic, ver, rec_cnt, addr_cnt, named_cnt = struct.unpack_from('<4sHIII', data, 0)
ts = struct.unpack_from('<Q', data, 18)[0]
region = data[26:72].rstrip(b'\x00').decode('utf-8', errors='replace')
sp_off, ni_off, ai_off, rec_off = struct.unpack_from('<IIII', data, 72)

# String pool
sp_count = struct.unpack_from('<I', data, sp_off)[0]
pool = []
pos = sp_off + 4
for _ in range(sp_count):
    slen = struct.unpack_from('<H', data, pos)[0]
    pos += 2
    s = data[pos:pos+slen].decode('utf-8', errors='replace')
    pool.append(s)
    pos += slen

# Find target string index
target_idx = None
for i, s in enumerate(pool):
    if s.lower() == search:
        target_idx = i
        break

if target_idx is None:
    print(f'"{search}" NOT FOUND in string pool')
    sys.exit(1)

print(f'"{search}" at pool index {target_idx}')

# Scan Address Index
print(f'\n=== Address Index ({addr_cnt} entries) ===')
pos = ai_off + 4
match_count = 0
sample_recs = []
for i in range(addr_cnt):
    city_idx = struct.unpack_from('<H', data, pos)[0]
    street_idx = struct.unpack_from('<H', data, pos+2)[0]
    hn_idx = struct.unpack_from('<H', data, pos+4)[0]
    rec_idx = struct.unpack_from('<I', data, pos+6)[0]
    pos += 10
    if city_idx == target_idx:
        match_count += 1
        if len(sample_recs) < 5:
            street = pool[street_idx] if street_idx < len(pool) else f'?{street_idx}'
            hn = pool[hn_idx] if hn_idx < len(pool) else f'?{hn_idx}'
            sample_recs.append((rec_idx, street, hn))

print(f'  Address entries with city="{search}": {match_count}')
for rec_idx, street, hn in sample_recs:
    print(f'    rec={rec_idx}  street={street!r}  hn={hn!r}')

# Scan Record Block for these sample recs
print(f'\n=== Sample Record Details ===')
pos = rec_off + 4
records = []
for i in range(rec_cnt):
    obj_type = data[pos]; pos += 1
    lat, lon = struct.unpack_from('<ff', data, pos); pos += 8
    if obj_type == 0:
        city_idx, street_idx, hn_idx = struct.unpack_from('<HHH', data, pos); pos += 6
        city = pool[city_idx] if city_idx < len(pool) else ''
        street = pool[street_idx] if street_idx < len(pool) else ''
        hn = pool[hn_idx] if hn_idx < len(pool) else ''
        records.append((i, 'Addr', lat, lon, city, street, hn))
    elif obj_type == 1:
        name_idx, translit_idx, cat = struct.unpack_from('<HHB', data, pos); pos += 5
        name = pool[name_idx] if name_idx < len(pool) else ''
        records.append((i, 'Named', lat, lon, name, '', ''))
    else:
        records.append((i, '?', 0.0, 0.0, '', '', ''))

# Show sample records
sample_rec_indices = {r[0] for r in sample_recs}
for (i, typ, lat, lon, *rest) in records:
    if i in sample_rec_indices:
        if typ == 'Addr':
            print(f'  rec[{i}] Addr: lat={lat:.4f} lon={lon:.4f} city={rest[0]!r} street={rest[1]!r} hn={rest[2]!r}')
        else:
            print(f'  rec[{i}] {typ}: lat={lat:.4f} lon={lon:.4f} name={rest[0]!r}')

# Named Index check
print(f'\n=== Named Index ({named_cnt} entries) ===')
pos = ni_off + 4
named_match = 0
for i in range(named_cnt):
    name_idx = struct.unpack_from('<H', data, pos)[0]
    pos += 9
    if name_idx == target_idx:
        named_match += 1
print(f'  Named entries with name="{search}": {named_match}')
