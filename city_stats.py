import struct, sys
from collections import Counter

with open(sys.argv[1], 'rb') as f:
    data = f.read()

sp_off = 88; n = struct.unpack_from('<I', data, sp_off)[0]
pool = []; pos = sp_off + 4
for _ in range(n):
    slen = struct.unpack_from('<H', data, pos)[0]; pos += 2
    pool.append(data[pos:pos+slen].decode('utf-8', errors='replace')); pos += slen

ai_off = struct.unpack_from('<I', data, 80)[0]  # addr_index_offset
ai_cnt = struct.unpack_from('<I', data, ai_off)[0]
cities = Counter()
pos = ai_off + 4
for _ in range(ai_cnt):
    city_idx = struct.unpack_from('<H', data, pos)[0]
    city = pool[city_idx] if city_idx < len(pool) else ''
    cities[city if city else '(пусто)'] += 1
    pos += 10

print(f'Всего адресов: {ai_cnt}')
print(f'Уникальных городов: {len(cities)}')
print()
for city, cnt in cities.most_common(30):
    print(f'  {cnt:>6}  {city}')
if len(cities) > 30:
    print(f'  ... и ещё {len(cities)-30}')
