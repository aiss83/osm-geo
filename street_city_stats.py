import struct, sys
from collections import Counter, defaultdict

path = sys.argv[1] if len(sys.argv) > 1 else '/tmp/test-kgd3.bin'

with open(path, 'rb') as f:
    data = f.read()

sp_off = struct.unpack_from('<I', data, 72)[0]
n = struct.unpack_from('<I', data, sp_off)[0]
pool = []; pos = sp_off + 4
for _ in range(n):
    slen = struct.unpack_from('<I', data, pos)[0]; pos += 4
    pool.append(data[pos:pos+slen].decode('utf-8', errors='replace')); pos += slen

ai_off = struct.unpack_from('<I', data, 80)[0]
ai_cnt = struct.unpack_from('<I', data, ai_off)[0]

# Статистика: город → количество улиц, город → улица → количество домов
city_streets = defaultdict(lambda: defaultdict(int))
city_addr_count = Counter()
empty_city = 0

pos = ai_off + 4
for _ in range(ai_cnt):
    city_idx = struct.unpack_from('<I', data, pos)[0]
    street_idx = struct.unpack_from('<I', data, pos+4)[0]
    city = pool[city_idx] if city_idx < len(pool) else ''
    street = pool[street_idx] if street_idx < len(pool) else ''
    if not city:
        empty_city += 1
    else:
        city_addr_count[city] += 1
        city_streets[city][street] += 1
    pos += 16

print(f"Всего адресов: {ai_cnt}")
print(f"Без города: {empty_city}")
print(f"С городом: {ai_cnt - empty_city}")
print(f"Уникальных городов: {len(city_addr_count)}")

print(f"\n=== Топ-15 городов по числу адресов ===")
for city, cnt in city_addr_count.most_common(15):
    street_cnt = len(city_streets[city])
    print(f"  {cnt:>6} адресов | {street_cnt:>4} улиц | {city}")

# Московский проспект — в каких городах
print(f"\n=== «Московский проспект» — привязка к городам ===")
msk = 'Московский проспект'
msk_var = 'Московский проспект съезд 1'
for city in sorted(city_streets.keys()):
    if msk in city_streets[city]:
        print(f"  {city}: {city_streets[city][msk]} домов")
    if msk_var in city_streets[city]:
        print(f"  {city}: {city_streets[city][msk_var]} домов (съезд 1)")

# Пример: топ улиц Калининграда
print(f"\n=== Топ-10 улиц Калининграда ===")
if 'Калининград' in city_streets:
    for street, cnt in sorted(city_streets['Калининград'].items(), key=lambda x: -x[1])[:10]:
        print(f"  {cnt:>4} домов | {street}")

# Пример: топ улиц Гурьевска
print(f"\n=== Топ-10 улиц Гурьевска ===")
if 'Гурьевск' in city_streets:
    for street, cnt in sorted(city_streets['Гурьевск'].items(), key=lambda x: -x[1])[:10]:
        print(f"  {cnt:>4} домов | {street}")
