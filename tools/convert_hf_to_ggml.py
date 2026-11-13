"""Convert a Hugging Face Whisper checkpoint (safetensors) to whisper.cpp GGML f16.

Port of whisper.cpp models/convert-h5-to-ggml.py (v1.8.3) without torch/transformers:
reads model.safetensors tensor by tensor with numpy, so no pickle and no remote code.

Usage:
  uv run --with numpy --with safetensors python -I convert_hf_to_ggml.py <hf_dir> <mel_filters.npz> <out.bin>
"""

import hashlib
import json
import struct
import sys
from pathlib import Path

import numpy as np
from safetensors import safe_open

CONV_MAP = {
    "self_attn.k_proj": "attn.key",
    "self_attn.q_proj": "attn.query",
    "self_attn.v_proj": "attn.value",
    "self_attn.out_proj": "attn.out",
    "self_attn_layer_norm": "attn_ln",
    "encoder_attn.q_proj": "cross_attn.query",
    "encoder_attn.v_proj": "cross_attn.value",
    "encoder_attn.out_proj": "cross_attn.out",
    "encoder_attn_layer_norm": "cross_attn_ln",
    "fc1": "mlp.0",
    "fc2": "mlp.2",
    "final_layer_norm": "mlp_ln",
    "encoder.layer_norm.bias": "encoder.ln_post.bias",
    "encoder.layer_norm.weight": "encoder.ln_post.weight",
    "encoder.embed_positions.weight": "encoder.positional_embedding",
    "decoder.layer_norm.bias": "decoder.ln.bias",
    "decoder.layer_norm.weight": "decoder.ln.weight",
    "decoder.embed_positions.weight": "decoder.positional_embedding",
    "decoder.embed_tokens.weight": "decoder.token_embedding.weight",
}

F32_NAMES = {
    "encoder.conv1.bias",
    "encoder.conv2.bias",
    "encoder.positional_embedding",
    "decoder.positional_embedding",
}


def bytes_to_unicode():
    bs = list(range(ord("!"), ord("~") + 1)) + list(range(ord("¡"), ord("¬") + 1)) + list(range(ord("®"), ord("ÿ") + 1))
    cs = bs[:]
    n = 0
    for b in range(2**8):
        if b not in bs:
            bs.append(b)
            cs.append(2**8 + n)
            n += 1
    return dict(zip(bs, [chr(c) for c in cs]))


def map_name(src: str) -> str | None:
    """HF tensor name -> whisper.cpp tensor name (None = skip)."""
    if src.endswith("proj_out.weight"):
        return None  # tied with decoder.embed_tokens, unused by whisper.cpp
    parts = src.split(".")
    if parts[0] == "model":
        parts = parts[1:]
    if len(parts) > 1 and parts[1] == "layers":
        parts[1] = "blocks"
        inner = ".".join(parts[3:-1])
        if inner == "encoder_attn.k_proj":
            mapped = "attn.key" if parts[0] == "encoder" else "cross_attn.key"
        else:
            mapped = CONV_MAP[inner]
        return ".".join(parts[:3] + [mapped] + parts[-1:])
    name = ".".join(parts)
    return CONV_MAP.get(name, name)


def main() -> None:
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    hf_dir, mel_path, out_path = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])

    hp = json.loads((hf_dir / "config.json").read_text(encoding="utf8"))
    max_length = hp.get("max_length") or hp.get("max_target_positions", 448)
    tokens = json.loads((hf_dir / "vocab.json").read_text(encoding="utf8"))
    n_mels = hp["num_mel_bins"]
    with np.load(mel_path, allow_pickle=False) as f:
        filters = f[f"mel_{n_mels}"].astype(np.float32)

    byte_decoder = {v: k for k, v in bytes_to_unicode().items()}
    out_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = out_path.with_suffix(".part")
    n_written = 0
    with open(tmp, "wb") as fout:
        fout.write(struct.pack("i", 0x67676D6C))
        for v in (
            hp["vocab_size"], hp["max_source_positions"], hp["d_model"], hp["encoder_attention_heads"],
            hp["encoder_layers"], int(max_length), hp["d_model"], hp["decoder_attention_heads"],
            hp["decoder_layers"], n_mels, 1,  # use_f16
        ):
            fout.write(struct.pack("i", v))

        fout.write(struct.pack("ii", *filters.shape))
        fout.write(filters.tobytes())

        fout.write(struct.pack("i", len(tokens)))
        for tok, _ in sorted(tokens.items(), key=lambda x: x[1]):
            text = bytes(byte_decoder[c] for c in tok)
            fout.write(struct.pack("i", len(text)))
            fout.write(text)

        with safe_open(hf_dir / "model.safetensors", framework="numpy") as st:
            for src in st.keys():
                name = map_name(src)
                if name is None:
                    print("skip", src)
                    continue
                data = st.get_tensor(src).squeeze()
                if name in ("encoder.conv1.bias", "encoder.conv2.bias"):
                    data = data.reshape(data.shape[0], 1)
                n_dims = data.ndim
                if n_dims < 2 or name in F32_NAMES:
                    data, ftype = data.astype(np.float32), 0
                else:
                    data, ftype = data.astype(np.float16), 1
                enc = name.encode("utf-8")
                fout.write(struct.pack("iii", n_dims, len(enc), ftype))
                for i in range(n_dims):
                    fout.write(struct.pack("i", data.shape[n_dims - 1 - i]))
                fout.write(enc)
                fout.write(np.ascontiguousarray(data).tobytes())
                n_written += 1

    tmp.replace(out_path)
    h = hashlib.sha256()
    with open(out_path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 24), b""):
            h.update(chunk)
    print(f"tensors: {n_written}")
    print(f"out: {out_path}")
    print(f"sha256: {h.hexdigest()}")


if __name__ == "__main__":
    main()
