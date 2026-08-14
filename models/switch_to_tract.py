"""Replace ort code with tract in normalizer.rs."""
c = open("src/normalizer.rs").read()

# Replace struct fields
old_struct = """    cache: HashMap<String, String>,
    /// ONNX encoder сессия (только с фичей `neural-normalizer`).
    #[cfg(feature = "neural-normalizer")]
    encoder: Option<ort::session::Session>,
    /// ONNX decoder сессия (только с фичей `neural-normalizer`).
    #[cfg(feature = "neural-normalizer")]
    decoder: Option<ort::session::Session>,"""

new_struct = """    cache: HashMap<String, String>,
    /// Пути к ONNX-моделям (только с фичей `neural-normalizer`).
    #[cfg(feature = "neural-normalizer")]
    encoder_path: Option<PathBuf>,
    #[cfg(feature = "neural-normalizer")]
    decoder_path: Option<PathBuf>,
    /// Кэш загруженных tract-моделей (загружаются лениво).
    #[cfg(feature = "neural-normalizer")]
    encoder_model: Option<SimplePlan<TypedModel>>,
    #[cfg(feature = "neural-normalizer")]
    decoder_model: Option<SimplePlan<TypedModel>>,"""

assert old_struct in c, f"OLD STRUCT not found in file"
c = c.replace(old_struct, new_struct)

# Replace the decoder_start_token_id/eos_token_id block (lines 189-193)
# Find and remove the block after decoder_model
old_token_block = """
    /// Token IDs for decoder start/end (mT5: """
# Find this pattern and delete lines until we find a line that doesn't start with #[cfg or a field
idx = c.find(old_token_block)
if idx >= 0:
    # Find the closing } of the struct
    end_idx = c.find("\n}", idx)
    if end_idx >= 0:
        # Keep the \n}
        c = c[:idx] + c[end_idx:]
        print("Removed decoder_start_token_id/eos_token_id block")

# Replace new() constructor
old_new = """            #[cfg(feature = "neural-normalizer")]
            encoder: None,
            #[cfg(feature = "neural-normalizer")]
            decoder: None,
            #[cfg(feature = "neural-normalizer")]
            decoder_start_token_id:"""
# Find and delete from encoder: None to the correct closing
idx = c.find(old_new)
if idx >= 0:
    # Find the next line that's not #[cfg or a field
    end_marker = "\n        }\n    }"
    end_idx = c.find(end_marker, idx)
    if end_idx >= 0:
        # find the start of the `}` for the Self block
        inner_end = c.rfind("        }", idx, end_idx)
        if inner_end >= 0:
            c = c[:idx] + c[inner_end:] 
            print("Fixed new() constructor")

# Fix the load() function body
old_load_body = """    #[cfg(feature = "neural-normalizer")]
    pub fn load(model_dir: &std::path::Path) -> anyhow::Result<Self> {
        let _ = model_dir;
        anyhow::bail!("ONNX-инференс отложен до стабилизации ort API (2.0.0-rc). Использую rule-based нормализатор.")
    }"""

new_load_body = """    #[cfg(feature = "neural-normalizer")]
    pub fn load(model_dir: &std::path::Path) -> anyhow::Result<Self> {
        use anyhow::Context;
        let ep = model_dir.join("normalizer_encoder.onnx");
        let dp = model_dir.join("normalizer_decoder.onnx");

        // Проверяем наличие файлов, но загружаем модели лениво
        if !ep.exists() || !dp.exists() {
            anyhow::bail!("ONNX-модели не найдены в {:?}", model_dir);
        }

        log::info!("ONNX-модели найдены (tract-onnx, pure Rust)");
        Ok(Self {
            cache: HashMap::new(),
            encoder_path: Some(ep),
            decoder_path: Some(dp),
            encoder_model: None,
            decoder_model: None,
        })
    }"""

assert old_load_body in c, f"OLD LOAD not found"
c = c.replace(old_load_body, new_load_body)

# Fix normalize() - references to self.encoder/self.decoder
old_norm = """        let result = if let (Some(encoder), Some(decoder)) =
            (&self.encoder, &self.decoder)"""

new_norm = """        let result = if self.encoder_path.is_some() && self.decoder_path.is_some() {"""

assert old_norm in c, f"OLD NORM not found"
c = c.replace(old_norm, new_norm)

# Fix the inner block: remove encoder/decoder references and add lazy loading
old_inner = """            // Нейросетевая нормализация: подаём rule-based результат,
            // нейросеть исправляет согласование прилагательных
            match self.run_onnx_inference(encoder, decoder, &rule_based) {"""

new_inner = """            // Нейросетевая нормализация: ленивая загрузка tract-моделей
            match self.run_tract_inference(&rule_based) {"""

assert old_inner in c, f"OLD INNER not found"
c = c.replace(old_inner, new_inner)

# Replace the run_onnx_inference function with run_tract_inference
old_fn_start = c.find("    fn run_onnx_inference(")
old_fn_end = c.find("\n    /// Батчевая", old_fn_start)
if old_fn_start >= 0 and old_fn_end > old_fn_start:
    new_fn = """    fn run_tract_inference(&mut self, text: &str) -> Result<String, anyhow::Error> {
        // Ленивая загрузка моделей при первом вызове
        if self.encoder_model.is_none() {
            let ep = self.encoder_path.as_ref().unwrap();
            let dp = self.decoder_path.as_ref().unwrap();
            log::info!("Загрузка tract ONNX моделей...");
            self.encoder_model = Some(
                tract_onnx::onnx()
                    .model_for_path(ep)?
                    .into_optimized()?
                    .into_runnable()?
            );
            self.decoder_model = Some(
                tract_onnx::onnx()
                    .model_for_path(dp)?
                    .into_optimized()?
                    .into_runnable()?
            );
            log::info!("tract модели загружены");
        }

        let encoder = self.encoder_model.as_ref().unwrap();
        let decoder = self.decoder_model.as_ref().unwrap();

        // SentencePiece токенизация
        #[cfg(feature = "sentencepiece-rs")]
        let sp = {
            let sp_path = std::path::Path::new("models/spiece.model");
            if sp_path.exists() {
                sentencepiece_rs::SentencePieceProcessor::open(sp_path)?
            } else {
                return Ok(text.to_string());
            }
        };
        #[cfg(not(feature = "sentencepiece-rs"))]
        { return Ok(text.to_string()); }

        let ids = sp.encode_to_ids(text)?;
        if ids.is_empty() { return Ok(text.to_string()); }

        // Encoder: input_ids [1, seq_len]
        let input: Tensor = tract_ndarray::Array1::from_iter(
            ids.iter().take(64).map(|&id| id as i64)
        ).into_shape((1, ids.len().min(64)))?;
        let input = input.into_tensor();

        let enc_result = encoder.run(tvec!(input.into()))?;
        let encoder_hidden = enc_result[0].to_array_view::<f32>()?.to_owned();

        // Decoder: autoregressive greedy
        let mut generated: Vec<i64> = vec![0]; // decoder_start_token_id = 0
        for _ in 0..64 {
            let dec_input: Tensor = tract_ndarray::Array2::from_shape_vec(
                (1, generated.len()),
                generated.clone(),
            )?.into();

            if let Ok(dec_result) = decoder.run(tvec!(dec_input.into(), encoder_hidden.clone().into_tensor().into())) {
                let logits = dec_result[0].to_array_view::<f32>()?;
                let last = logits.slice(ndarray::s![0, -1, ..]);
                let next = last.iter().enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i as i64)
                    .unwrap_or(1);
                if next == 1 { break; } // eos_token_id = 1
                generated.push(next);
            } else {
                break;
            }
        }

        let decoded: Vec<usize> = generated.iter().map(|&id| id as usize).collect();
        let result = sp.decode_ids(&decoded)?;
        Ok(if result.trim().is_empty() { text.to_string() } else { result })
    }

"""
    c = c[:old_fn_start] + new_fn + c[old_fn_end:]
    print("Replaced run_onnx_inference with run_tract_inference")

# Write back
open("src/normalizer.rs", "w").write(c)
print("Done - all replacements applied")
