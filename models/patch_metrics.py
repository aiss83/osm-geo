"""Fix train.py: replace compute_metrics with safe version that handles OverflowError."""
with open("models/train.py", "r") as f:
    content = f.read()

old = """def compute_metrics(eval_pred, tokenizer):
    \"\"\"Вычислить BLEU и Exact Match.\"\"\"
    predictions, labels = eval_pred

    # Декодируем predictions
    if isinstance(predictions, tuple):
        predictions = predictions[0]  # для Seq2Seq

    decoded_preds = tokenizer.batch_decode(predictions, skip_special_tokens=True)
    decoded_labels = tokenizer.batch_decode(labels, skip_special_tokens=True)"""

new = """def compute_metrics(eval_pred, tokenizer):
    \"\"\"Вычислить BLEU и Exact Match.\"\"\"
    import numpy as np
    predictions, labels = eval_pred

    if isinstance(predictions, tuple):
        predictions = predictions[0]

    def safe_decode(ids, tok):
        vocab_size = tok.vocab_size
        pad_id = tok.pad_token_id or 0
        ids = np.where((ids >= 0) & (ids < vocab_size), ids, pad_id)
        return tok.batch_decode(ids, skip_special_tokens=True)

    decoded_preds = safe_decode(predictions, tokenizer)
    decoded_labels = safe_decode(labels, tokenizer)"""

if old in content:
    content = content.replace(old, new)
    with open("models/train.py", "w") as f:
        f.write(content)
    print("OK: compute_metrics patched")
else:
    print("BLOCK NOT FOUND, trying fuzzy match...")
    # Find the function boundary
    start = content.find("def compute_metrics(eval_pred, tokenizer):")
    if start >= 0:
        print(f"  Found at offset {start}")
        # Print context around it
        print(content[start:start+500])
    else:
        print("  Function not found at all!")
