import struct, sys, glob

for path in sys.argv[1:]:
    with open(path, 'rb') as f:
        data = f.read()
    
    sp_off = struct.unpack_from('<I', data, 72)[0]
    sp_count = struct.unpack_from('<I', data, sp_off)[0]
    pool = []
    pos = sp_off + 4
    for _ in range(sp_count):
        slen = struct.unpack_from('<I', data, pos)[0]
        pos += 4
        s = data[pos:pos+slen].decode('utf-8', errors='replace')
        pool.append(s)
        pos += slen
    
    isakovo = [s for s in pool if 'исаково' in s.lower()]
    isakova = [s for s in pool if 'исакова' in s.lower()]
    prospekt = [s for s in pool if 'проспекту' in s.lower()]
    
    print(f"\n=== {path} ===")
    if isakovo:
        print(f"  Исаково ({len(isakovo)}): {isakovo}")
    else:
        print("  Исаково: NONE")
    if isakova:
        print(f"  Исакова ({len(isakova)}): {isakova}")
    else:
        print("  Исакова: NONE")
    if prospekt:
        print(f"  проспекту ({len(prospekt)}): {prospekt}")
    else:
        print("  проспекту: NONE")
