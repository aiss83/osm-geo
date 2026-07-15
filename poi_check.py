import struct, sys
from collections import Counter

with open(sys.argv[1], 'rb') as f:
    d = f.read()

sp_off = 88; n = struct.unpack_from('<I', d, sp_off)[0]
pool = []; pos = sp_off + 4
for _ in range(n):
    slen = struct.unpack_from('<H', d, pos)[0]; pos += 2
    pool.append(d[pos:pos+slen].decode('utf-8', errors='replace')); pos += slen

ni_off = struct.unpack_from('<I', d, 76)[0]
ni_cnt = struct.unpack_from('<I', d, ni_off)[0]
cats = Counter()
place_names = []
pos = ni_off + 4
for _ in range(ni_cnt):
    name_idx = struct.unpack_from('<H', d, pos)[0]
    cat = struct.unpack_from('<B', d, pos+4)[0]
    name = pool[name_idx] if name_idx < len(pool) else ''
    cats[cat] += 1
    pos += 9

cat_names = {0:'None', 1:'amenity', 2:'tourism', 3:'shop', 4:'historic',
             5:'leisure', 6:'office', 7:'boundary', 8:'highway'}

print(f'Total Named POI: {ni_cnt}')
print()
for cat, cnt in sorted(cats.items()):
    pct = 100.0 * cnt / ni_cnt
    print(f'  {cat_names.get(cat, str(cat)):>10}: {cnt:>5} ({pct:.1f}%)')

# Check for place names that look like populated places
print(f'\n=== POI matching known city names ===')
# Collect city names from address index
ai_off = struct.unpack_from('<I', d, 80)[0]
ai_cnt = struct.unpack_from('<I', d, ai_off)[0]
city_names = set()
pos = ai_off + 4
for _ in range(ai_cnt):
    city_idx = struct.unpack_from('<H', d, pos)[0]
    city = pool[city_idx] if city_idx < len(pool) else ''
    if city:
        city_names.add(city)
    pos += 10

# Check named index for matches
pos = ni_off + 4
poi_cities = []
for _ in range(ni_cnt):
    name_idx = struct.unpack_from('<H', d, pos)[0]
    cat = struct.unpack_from('<B', d, pos+4)[0]
    name = pool[name_idx] if name_idx < len(pool) else ''
    if name in city_names:
        poi_cities.append((name, cat_names.get(cat, str(cat))))
    pos += 9

if poi_cities:
    print(f'  {len(poi_cities)} POI совпадают с городами из Address Index:')
    for name, cat in poi_cities[:20]:
        print(f'    {name} ({cat})')
    if len(poi_cities) > 20:
        print(f'    ... и ещё {len(poi_cities)-20}')
else:
    print('  НИ ОДНОГО совпадения — place nodes отфильтрованы')
