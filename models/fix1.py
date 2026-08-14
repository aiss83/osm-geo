"""Step 1: Replace ort types in normalizer.rs"""
c = open("src/normalizer.rs").read()
c = c.replace("ort::session::Session", "SimplePlan<TypedModel>")

# Replace load() body with tract-based version
old_load = '        let _ = model_dir;\n        anyhow::bail!(\"ONNX'
idx = c.find(old_load)
assert idx >= 0, f"old_load not found at all"

# Find end of load() body
closing = c.find("    }\n\n    /// Нормализовать", idx)
assert closing >= 0, "closing not found"

new_load = '        let ep = model_dir.join(\"normalizer_encoder.onnx\");\n        let dp = model_dir.join(\"normalizer_decoder.onnx\");\n        if !ep.exists() || !dp.exists() { anyhow::bail!(\"ONNX models not found in {:?}\", model_dir); }\n        log::info!(\"ONNX models found (tract, pure Rust)\");\n        Ok(Self { cache: HashMap::new(), encoder: None, decoder: None, decoder_start_token_id: 0, eos_token_id: 1 })'

c = c[:idx] + new_load + c[closing:]
open("src/normalizer.rs", "w").write(c)
print("OK: step1 done")
