"""Fix normalizer.rs: replace load() body with stub."""
c = open("src/normalizer.rs").read()
old_start = c.find("    pub fn load(model_dir:")
old_end = c.find("    /// Нормализовать одно название.")
if old_start < 0 or old_end < 0:
    print("ERROR: cannot find boundaries")
    exit(1)
c = c[:old_start] + '    pub fn load(model_dir: &std::path::Path) -> anyhow::Result<Self> {\n        let _ = model_dir;\n        anyhow::bail!("ONNX-инференс отложен до стабилизации ort API (2.0.0-rc). Использую rule-based нормализатор.")\n    }\n\n' + c[old_end:]
open("src/normalizer.rs", "w").write(c)
print("OK")
