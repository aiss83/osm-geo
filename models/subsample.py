#!/usr/bin/env python3
"""Subsample dataset for quick CPU training."""
import json, random, sys

random.seed(42)

for split, count in [("train", 10000), ("val", 1000)]:
    path = f"models/data/{split}.jsonl"
    pairs = [json.loads(l) for l in open(path, encoding="utf-8")]
    random.shuffle(pairs)
    sample = pairs[:count]
    out = f"models/data/{split}_small.jsonl"
    with open(out, "w", encoding="utf-8") as f:
        for p in sample:
            f.write(json.dumps(p, ensure_ascii=False) + "\n")
    print(f"{split}: {len(sample)}/{len(pairs)} examples -> {out}")
