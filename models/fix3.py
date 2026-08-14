"""Fix all remaining tract issues in normalizer.rs"""
c = open("src/normalizer.rs").read()

# 1. Fix gen keyword (Rust 2024) -> generated
c = c.replace("gen.", "generated.")
c = c.replace("gen:", "generated:")
c = c.replace("let mut gen:", "let mut generated:")
c = c.replace("let mut gen ", "let mut generated ")
c = c.replace("gen.push", "generated.push")
c = c.replace("gen.iter", "generated.iter")
c = c.replace("gen.len", "generated.len")
c = c.replace("gen.clone", "generated.clone")
c = c.replace("Vec<i64> = gen", "Vec<i64> = generated")
# Also fix the variable declaration
c = c.replace("let mut generated: Vec<i64> = vec![0];", "let mut gen_ids: Vec<i64> = vec![0];")

# But wait, we already replaced gen -> generated. Let me fix the vec![0] line properly
c = c.replace("let mut generated: Vec<i64> = vec![0];", "let mut gen_ids: Vec<i64> = vec![0];")
# Now fix references that were changed to generated to use gen_ids instead
# Actually, let me use a different variable name altogether to avoid confusion
c = c.replace("generated: Vec<i64>", "token_ids_out: Vec<i64>")
c = c.replace("let mut generated", "let mut token_ids_out")
c = c.replace("generated.push", "token_ids_out.push")
c = c.replace("generated.iter", "token_ids_out.iter")
c = c.replace("generated.len", "token_ids_out.len")
c = c.replace("generated.clone", "token_ids_out.clone")
c = c.replace("&generated", "&token_ids_out")
c = c.replace("generated,", "token_ids_out,")

# 2. Fix SimplePlan not in scope - add explicit import
old_import = "#[cfg(feature = \"neural-normalizer\")]\nuse tract_onnx::prelude::*;"
new_import = "#[cfg(feature = \"neural-normalizer\")]\nuse tract_onnx::prelude::*;\n#[cfg(feature = \"neural-normalizer\")]\nuse tract_onnx::tract_core::plan::SimplePlan;"
c = c.replace(old_import, new_import)

# 3. Fix struct fields - check current state
# The struct should have encoder_path etc. Let me check what's there now
import re
struct_start = c.find("pub struct Normalizer {")
struct_end = c.find("\n}", struct_start)
struct_block = c[struct_start:struct_end]
print(f"Current struct: {struct_block[:200]}...")

# If struct still has old fields, fix it
if "encoder: Option" in struct_block and "encoder_path" not in struct_block:
    # Replace old fields with new ones
    old = "    encoder: Option<SimplePlan<TypedModel>>,\n    /// ONNX decoder"
    new = "    encoder_path: Option<PathBuf>,\n    decoder_path: Option<PathBuf>,\n    encoder_model: Option<SimplePlan<TypedModel>>,\n    decoder_model: Option<SimplePlan<TypedModel>>,\n    /// ONNX decoder"
    c = c.replace(old, new)
    
    # Remove old decoder field
    c = c.replace("    decoder: Option<SimplePlan<TypedModel>>,\n", "")
    
# Fix new() constructor
c = c.replace("encoder: None,\n            decoder: None,\n            decoder_start_token_id: 0,\n            eos_token_id: 1",
              "encoder_path: None,\n            decoder_path: None,\n            encoder_model: None,\n            decoder_model: None,\n            decoder_start_token_id: 0,\n            eos_token_id: 1")

# Fix the token_ids_out variable name back to something cleaner
c = c.replace("token_ids_out", "gen_ids")

# Fix the decode line
c = c.replace("let decoded: Vec<usize> = gen_ids.iter().map(|&id| id as usize).collect();",
              "let decoded_ids: Vec<usize> = gen_ids.iter().map(|&id| id as usize).collect();")
c = c.replace("sp.decode_ids(&decoded)?", "sp.decode_ids(&decoded_ids)?")

open("src/normalizer.rs", "w").write(c)
print("OK: fix3 done")
