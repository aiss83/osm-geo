import struct, sys

with open(sys.argv[1], 'rb') as f:
    d = f.read()

target = sys.argv[2] if len(sys.argv) > 2 else 'Калининградский зоопарк'

# Find in string pool
sp_off = 88; n = struct.unpack_from('<I', d, sp_off)[0]
pool = []; pos = sp_off + 4; target_idx = None
for i in range(n):
    slen = struct.unpack_from('<H', d, pos)[0]; pos += 2
    s = d[pos:pos+slen].decode('utf-8', errors='replace')
    pool.append(s); pos += slen
    if s == target:
        target_idx = i

if target_idx is None:
    print(f'"{target}" not found')
    sys.exit(1)

# Find in Named Index
ni_off = struct.unpack_from('<I', d, 76)[0]
pos = ni_off + 4
ni_cnt = struct.unpack_from('<I', d, ni_off)[0]
rec_idx = None; cat = None
for _ in range(ni_cnt):
    name_idx = struct.unpack_from('<H', d, pos)[0]
    if name_idx == target_idx:
        cat = struct.unpack_from('<B', d, pos+4)[0]
        rec_idx = struct.unpack_from('<I', d, pos+5)[0]
        break
    pos += 9

if rec_idx is None:
    print(f'"{target}" not in Named Index')
    sys.exit(1)

# Read record — scan sequentially (variable record size!)
rec_off = struct.unpack_from('<I', d, 84)[0]
rec_cnt = struct.unpack_from('<I', d, rec_off)[0]
pos = rec_off + 4
for i in range(rec_cnt):
    obj_type = d[pos]; pos += 1
    lat, lon = struct.unpack_from('<ff', d, pos); pos += 8
    if obj_type == 0:  # Address
        pos += 6  # city_idx(2) + street_idx(2) + hn_idx(2)
    elif obj_type == 1:  # Named
        pos += 5  # name_idx(2) + translit_idx(2) + cat(1)
    if i == rec_idx:
        break

cat_names = {0:'None', 1:'amenity', 2:'tourism', 3:'shop', 4:'historic'}
print(f'name: {target}')
print(f'pool idx: {target_idx}')
print(f'record idx: {rec_idx}')
print(f'category: {cat_names.get(cat, str(cat))} ({cat})')
print(f'lat: {lat:.6f}')
print(f'lon: {lon:.6f}')
