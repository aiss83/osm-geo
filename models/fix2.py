"""Step 2: Fix normalize() and run_onnx_inference in normalizer.rs"""
c = open("src/normalizer.rs").read()

# Fix the broken load() closing line
c = c.replace('})    }', '})\n    }')

# Fix normalize() to use self.encoder_path instead of self.encoder/self.decoder
old_norm = '        let result = if let (Some(encoder), Some(decoder)) =\n            (&self.encoder, &self.decoder)'
new_norm = '        let result = if self.encoder_path.is_some() && self.decoder_path.is_some()'
assert old_norm in c, f"old normalize pattern not found"
c = c.replace(old_norm, new_norm)

# Fix the inner call from run_onnx_inference(encoder, decoder, ...) to run_tract_inference(...)
old_call = '            match self.run_onnx_inference(encoder, decoder, &rule_based) {'
new_call = '            match self.run_tract_inference(&rule_based) {'
assert old_call in c, f"old call not found"
c = c.replace(old_call, new_call)

# Now replace run_onnx_inference with run_tract_inference
old_fn = '    fn run_onnx_inference(\n        &self,\n        _encoder: &SimplePlan<TypedModel>,\n        _decoder: &SimplePlan<TypedModel>,\n        text: &str,\n    ) -> Result<String, anyhow::Error> {\n        // Stub: ONNX-инференс отложен до стабилизации ort API (2.0.0-rc).\n        // Нормализатор применяет rule-based fallback.\n        let _ = text;\n        Ok(text.to_string())\n    }'

new_fn = """    fn run_tract_inference(&mut self, text: &str) -> Result<String, anyhow::Error> {
        use tract_onnx::prelude::*;

        // Lazy-load tract models on first call
        if self.encoder_model.is_none() {
            let ep = self.encoder_path.as_ref().unwrap();
            let dp = self.decoder_path.as_ref().unwrap();
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
        }

        let encoder = self.encoder_model.as_ref().unwrap();
        let decoder = self.decoder_model.as_ref().unwrap();

        // SentencePiece tokenizer
        #[cfg(feature = "sentencepiece-rs")]
        let sp = {
            let sp_path = std::path::Path::new("models/spiece.model");
            if sp_path.exists() { sentencepiece_rs::SentencePieceProcessor::open(sp_path)? }
            else { return Ok(text.to_string()); }
        };
        #[cfg(not(feature = "sentencepiece-rs"))]
        { return Ok(text.to_string()); }

        let token_ids = sp.encode_to_ids(text)?;
        if token_ids.is_empty() { return Ok(text.to_string()); }
        let ids: Vec<i64> = token_ids.into_iter().take(64).map(|id| id as i64).collect();

        // Encoder
        let arr = tract_ndarray::Array2::from_shape_vec((1, ids.len()), ids.clone())?;
        let enc_out = encoder.run(tvec!(arr.into_tensor().into()))?;
        let hidden = enc_out[0].to_array_view::<f32>()?.to_owned();

        // Decoder: autoregressive greedy
        let mut gen: Vec<i64> = vec![0];
        for _ in 0..64 {
            let din = tract_ndarray::Array2::from_shape_vec((1, gen.len()), gen.clone())?;
            if let Ok(dout) = decoder.run(tvec!(din.into_tensor().into(), hidden.clone().into_tensor().into())) {
                let logits = dout[0].to_array_view::<f32>()?;
                let nxt = logits.slice(ndarray::s![0, -1, ..]).iter().enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i as i64).unwrap_or(1);
                if nxt == 1 { break; }
                gen.push(nxt);
            } else { break; }
        }

        let decoded: Vec<usize> = gen.iter().map(|&id| id as usize).collect();
        let result = sp.decode_ids(&decoded)?;
        Ok(if result.trim().is_empty() { text.to_string() } else { result })
    }"""

assert old_fn in c, f"old_fn not found"
c = c.replace(old_fn, new_fn)

# Add encoder_path/decoder_path/encoder_model/decoder_model fields to struct
# Replace the old encoder/decoder fields with new ones
s1 = '    encoder: Option<SimplePlan<TypedModel>>,'
s2 = '    decoder: Option<SimplePlan<TypedModel>>,'
s3 = '    decoder_start_token_id: i64,'
s4 = '    eos_token_id: i64,'
assert s1 in c and s2 in c and s3 in c and s4 in c, "struct fields not found"

new_fields = """    encoder_path: Option<PathBuf>,
    decoder_path: Option<PathBuf>,
    encoder_model: Option<SimplePlan<TypedModel>>,
    decoder_model: Option<SimplePlan<TypedModel>>,
    decoder_start_token_id: i64,
    eos_token_id: i64,"""
c = c.replace(s1 + '\n    ' + s2 + '\n    ' + s3 + '\n    ' + s4, new_fields)

# Fix new() constructor fields
c = c.replace('encoder: None,\n            decoder: None,\n            decoder_start_token_id: 0,\n            eos_token_id: 1',
              'encoder_path: None,\n            decoder_path: None,\n            encoder_model: None,\n            decoder_model: None,\n            decoder_start_token_id: 0,\n            eos_token_id: 1')

# Fix load() constructor
c = c.replace('encoder: None, decoder: None, decoder_start_token_id: 0, eos_token_id: 1',
              'encoder_path: Some(ep), decoder_path: Some(dp), encoder_model: None, decoder_model: None, decoder_start_token_id: 0, eos_token_id: 1')

open("src/normalizer.rs", "w").write(c)
print("OK: step2 done")
