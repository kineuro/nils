# Spikes

Throwaway code that answers a question in the design record. Each spike is a directory with a `README.md` that states the question, the criteria written down before the work started, and the verdict when it is in. Nothing here is shipped, depended on, or held to the engine's quality bar; the verdict is the product.

| Spike | Question | Criteria | Opened |
|---|---|---|---|
| [`lang/`](lang/) | Which language does the engine use: Rust, the prior, or Go (C1, D2)? | Throughput and memory on one million instances, vendor-file failures, static cross-compiled binaries, and maintainability. Ten working days. | 2026-09-02 |
| [`stacks/`](stacks/) | Does v1 partition a series into stacks the way v0 did (Wave 1, §14 item 5)? | Over the series both registries hold, the share whose partitions are equal, and for the rest, the signature field that caused the difference. | 2026-09-02 |
