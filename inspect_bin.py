import struct, sys

with open(sys.argv[1], 'rb') as f:
    data = f.read()

# Parse header
magic, version, rec_count, addr_cnt, named_cnt = struct.unpack_from('<4sHIII', data, 0)
ts = struct.unpack_from('<Q', data, 18)[0]
region = data[26:72].rstrip(b'\x00').decode('utf-8', errors='replace')
sp_off, ni_off, ai_off, rec_off = struct.unpack_from('<IIII', data, 72)

print(f"Magic: {magic}, Version: {version}")
print(f"Records: {rec_count}, Addr: {addr_cnt}, Named: {named_cnt}")
print(f"Region: '{region}', Timestamp: {ts}")
print(f"Offsets: SP={sp_off}, NI={ni_off}, AI={ai_off}, REC={rec_off}")

# Read string pool
sp_count = struct.unpack_from('<I', data, sp_off)[0]
pool = []
pos = sp_off + 4
for i in range(sp_count):
    slen = struct.unpack_from('<H', data, pos)[0]
    pos += 2
    s = data[pos:pos+slen].decode('utf-8', errors='replace')
    pool.append(s)
    pos += slen
print(f"\nString pool: {sp_count} strings, pool ended at byte {pos}, NI starts at {ni_off}")

# Read Named Index
ni_count = struct.unpack_from('<I', data, ni_off)[0]
print(f"\nNamed Index: {ni_count} entries")
pos = ni_off + 4
for i in range(min(ni_count, 5)):
    name_idx, cat, rec_idx = struct.unpack_from('<HBI', data, pos)
    name = pool[name_idx] if name_idx < len(pool) else f"OOB({name_idx})"
    print(f"  [{i}]: name={name_idx}='{name}', cat={cat}, rec_idx={rec_idx}")
    pos += 7

# Read Address Index
ai_count = struct.unpack_from('<I', data, ai_off)[0]
print(f"\nAddress Index: {ai_count} entries")
pos = ai_off + 4
for i in range(min(ai_count, 5)):
    city_idx, street_idx, hn_idx, rec_idx = struct.unpack_from('<HHHI', data, pos)
    city = pool[city_idx] if city_idx < len(pool) else f"OOB({city_idx})"
    street = pool[street_idx] if street_idx < len(pool) else f"OOB({street_idx})"
    hn = pool[hn_idx] if hn_idx < len(pool) else f"OOB({hn_idx})"
    print(f"  [{i}]: city={city_idx}='{city}', street={street_idx}='{street}', hn={hn_idx}='{hn}', rec_idx={rec_idx}")
    pos += 10

# Read Record Block
rec_count = struct.unpack_from('<I', data, rec_off)[0]
print(f"\nRecord Block: {rec_count} entries")
pos = rec_off + 4
for i in range(min(rec_count, 10)):
    obj_type = data[pos]; pos += 1
    lat, lon = struct.unpack_from('<ff', data, pos); pos += 8
    if obj_type == 0:
        city_idx, street_idx, hn_idx = struct.unpack_from('<HHH', data, pos); pos += 6
        city = pool[city_idx] if city_idx < len(pool) else f"OOB({city_idx})"
        print(f"  [{i}] Addr: lat={lat:.4f} lon={lon:.4f} city={city_idx}='{city}' street={street_idx} hn={hn_idx}")
    elif obj_type == 1:
        country_idx, city_idx, name_idx, cat = struct.unpack_from('<HHHB', data, pos); pos += 7
        country = pool[country_idx] if country_idx < len(pool) else f"OOB({country_idx})"
        city = pool[city_idx] if city_idx < len(pool) else f"OOB({city_idx})"
        name = pool[name_idx] if name_idx < len(pool) else f"OOB({name_idx})"
        print(f"  [{i}] Named: lat={lat:.4f} lon={lon:.4f} country={country_idx}='{country}' city={city_idx}='{city}' name={name_idx}='{name}' cat={cat}")
    else:
        print(f"  [{i}] UNKNOWN type={obj_type}")


