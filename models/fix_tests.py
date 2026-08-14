"""Fix test compilation: cfg-gate new fields in Normalizer::new()"""
c = open("src/normalizer.rs").read()

# 1. Fix cfg gates in new() constructor
old = "            cache: HashMap::new(),\n            #[cfg(feature = \"neural-normalizer\")]\n            encoder_path: None,\n            #[cfg(feature = \"neural-normalizer\")]\n            decoder_path: None,\n            #[cfg(feature = \"neural-normalizer\")]\n            encoder_model: None,\n            #[cfg(feature = \"neural-normalizer\")]\n            decoder_model: None,\n            #[cfg(feature = \"neural-normalizer\")]\n            decoder_start_token_id: 0,\n            #[cfg(feature = \"neural-normalizer\")]\n            eos_token_id: 1,"
new = "            cache: HashMap::new(),\n            #[cfg(feature = \"neural-normalizer\")]\n            {\n                encoder_path: None,\n                decoder_path: None,\n                encoder_model: None,\n                decoder_model: None,\n                decoder_start_token_id: 0,\n                eos_token_id: 1,\n            }"

if old in c:
    c = c.replace(old, new)
    print("Fixed cfg gates in new()")
else:
    print(f"OLD not found, checking alternatives...")
    # Try simpler approach: remove individual cfg and use block
    if "encoder_path: None" in c:
        print("  encoder_path: None found in file")
    if "decoder_start_token_id: 0" in c:
        print("  decoder_start_token_id: 0 found in file")

# 2. Check ABBREVIATIONS
if "ABBREVIATIONS" in c:
    print("ABBREVIATIONS found in file")
else:
    print("ABBREVIATIONS MISSING - need to investigate")

open("src/normalizer.rs", "w").write(c)
print("Done")
