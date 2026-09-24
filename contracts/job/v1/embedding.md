<!-- SPDX-License-Identifier: Apache-2.0 -->

# The embedding file

Record 43 S4. An embedding is one encoder's features for one stack: the
slices it was made from and a float32 matrix with one row per slice. The
engine keeps it as a derivative of kind `embedding`, keyed by the stack,
the encoder's model id and the preprocessing version, and a pipeline image
writes and reads it in this format. The media type is
`application/vnd.nils.embedding` and the extension `.emb`.

## Layout

| offset | bytes | what |
|---|---|---|
| 0 | 8 | the magic `NILSEMB1`, ASCII; the `1` is the format version |
| 8 | 4 | `H`, the length of the header in bytes, unsigned little-endian |
| 12 | `H` | the header, UTF-8 JSON |
| 12 + `H` | 0 to 63 | zero bytes, so the matrix starts at a multiple of 64 |
| `D` | 4 × rows × dim | the matrix, little-endian float32, row after row |

The file ends where the matrix ends. The header holds:

| key | what |
|---|---|
| `format` | `nils-embedding` |
| `stack_id` | the stack, by the id the run's `stacks.json` gave it |
| `encoder` | the encoder's weight digest, `sha256:` and 64 lowercase hex digits, as its model row is registered |
| `preprocess_version` | the preparation the slices went through, a word without spaces; a new one is a new key, and every stack is embedded again |
| `rows` | the number of rows |
| `dim` | the width of a row |
| `dtype` | `<f4` |
| `slices` | one index per row, in row order: the slice (the frame within the stack, from 0) the row was made from, each once |

Every value is finite. A reader refuses a file whose length, rows, slices
or padding disagree.

## Why not `.npy`

An `.npy` file holds one array, so the slice indices and the provenance
would need a second file or a structured dtype, whose header is a Python
literal the engine would have to parse. This is one file that says what it
is, in JSON that both sides read, and its matrix is exactly what
`numpy.frombuffer` maps without a copy.

## Writing and reading it in Python

```python
import json, struct
import numpy as np

def write(path, stack_id, encoder, preprocess_version, slices, matrix):
    m = np.ascontiguousarray(matrix, dtype="<f4")
    assert m.ndim == 2 and m.shape[0] == len(slices) and np.isfinite(m).all()
    header = json.dumps({
        "format": "nils-embedding", "stack_id": stack_id, "encoder": encoder,
        "preprocess_version": preprocess_version, "rows": m.shape[0],
        "dim": m.shape[1], "dtype": "<f4", "slices": [int(s) for s in slices],
    }, separators=(",", ":")).encode()
    pad = -(12 + len(header)) % 64
    with open(path, "wb") as f:
        f.write(b"NILSEMB1" + struct.pack("<I", len(header)) + header + b"\0" * pad)
        f.write(m.tobytes())

def read(path):
    b = open(path, "rb").read()
    assert b[:8] == b"NILSEMB1"
    (n,) = struct.unpack_from("<I", b, 8)
    header = json.loads(b[12:12 + n])
    start = 12 + n + (-(12 + n) % 64)
    m = np.frombuffer(b, "<f4", offset=start).reshape(header["rows"], header["dim"])
    return header, m
```

## The cache

The runner asks the engine which of a run's stacks have an embedding under
the encoder and preprocessing version already, mounts those read-only, and
hands the pipeline only the rest. A pipeline writes one file per stack it
embeds; an output for a stack the key already holds is not registered, and
the row that was there stays, since two devices do not give the same bytes
(record 43 R5). A preprocessing version the engine has not seen embeds
every stack again, and the rows of the earlier version stay beside the new
ones.
