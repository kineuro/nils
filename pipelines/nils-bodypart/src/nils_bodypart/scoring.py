# SPDX-License-Identifier: AGPL-3.0-only
"""The certified body-part model over many stacks at once (record 50).

A stack's time goes into reading its files' headers and decoding its frames
(pure Python and a JPEG 2000 decoder), not into the network, so a run scores
stacks in worker processes, as many as it has threads, each with the model
loaded once and one thread of its own for numpy and LightGBM. A worker
takes a few stacks at a time and asks the kernel to read the next stack's
files ahead while it builds the current one, so a network file system's
latency overlaps the work. The answers come back in the manifest's order.

With a CUDA card (``device`` ``cuda``, or ``auto`` where one is found) each
worker decodes lossless JPEG 2000 on the card (``gpu.J2KDecoder``); the
numbers after decoding are the CPU path's. ``encoder_device`` ``cuda`` also
moves the image encoder onto the card, over batches of stacks, in the main
process (``gpu.TorchEncoder``); that one is not bit for bit the numpy
encoder, so the default keeps the encoder on the CPU.

Each stack gives ``(stack, result, why)``: the ``fusion.StackResult``, or
None and the unit's status and error, exactly as the serial path gave them.
"""

from __future__ import annotations

import logging
import multiprocessing as mp
import os
from collections import deque
from collections.abc import Iterator
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

from . import fusion, manifest, volume
from .results import FAILED, SKIPPED

logger = logging.getLogger("nils_bodypart")

# the environment each worker starts with: one thread for each library
ONE_THREAD = {k: "1" for k in ("OMP_NUM_THREADS", "OPENBLAS_NUM_THREADS", "MKL_NUM_THREADS", "NUMEXPR_NUM_THREADS")}

_W: dict = {}


def outcome(exc: BaseException) -> tuple[str, str]:
    """A stack that got no answer: its unit's status and error, never a path."""
    if isinstance(exc, fusion.NoFingerprint):
        return SKIPPED, "no fingerprint row: the model abstains"
    if isinstance(exc, volume.NoGeometry):
        return SKIPPED, "no geometry (position, orientation and pixel spacing): the model abstains"
    if isinstance(exc, volume.Unreadable):
        return FAILED, "no file of the stack could be read"
    logger.warning("a stack failed: %s", type(exc).__name__)
    return FAILED, f"the stack could not be scored ({type(exc).__name__})"


def readahead(st: manifest.Stack) -> None:
    """Ask the kernel to read a stack's files ahead (it returns at once)."""
    for path, _ in st.files:
        try:
            fd = os.open(path, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_WILLNEED)
        except OSError:
            pass
        finally:
            os.close(fd)


def one_thread():
    """Hold numpy's BLAS and LightGBM's OpenMP to one thread in this process.
    A BLAS matrix product's last bits depend on how many threads share it,
    so this is also what makes a stack's numbers the same whatever the
    run's threads and the machine's cores."""
    from threadpoolctl import threadpool_limits

    return threadpool_limits(limits=1)


def _init(inputs: str, gpu_decode: bool, prepare_only: bool, prefetch: bool, started=None, gpu_workers: int = 0) -> None:
    one_thread()
    _W["model"] = None if prepare_only else fusion.load(Path(inputs))
    _W["prepare_only"], _W["prefetch"] = prepare_only, prefetch
    _W["decode"] = None
    if gpu_decode and started is not None:
        # only the first gpu_workers workers decode on the card: a card is
        # one queue, and more processes on it only wait on each other
        with started.get_lock():
            gpu_decode = started.value < gpu_workers
            started.value += 1
    if gpu_decode:
        try:
            from .gpu import J2KDecoder

            _W["decode"] = J2KDecoder()
        except Exception as e:  # noqa: BLE001 - the CPU decodes instead
            logger.warning("the GPU decoder did not start (%s); decoding on the CPU", type(e).__name__)


def _one(st: manifest.Stack):
    try:
        if _W["prepare_only"]:
            return fusion.prepare(st.files, st.extra.get("header"), _W["decode"]), None
        return fusion.predict(_W["model"], st.files, st.extra.get("header"), _W["decode"]), None
    except Exception as e:  # noqa: BLE001 - a unit fails, the run goes on
        return None, outcome(e)


def _chunk(stacks: list[manifest.Stack]):
    from .gpu import J2KDecoder

    before = J2KDecoder.stats()
    out = []
    for i, st in enumerate(stacks):
        if _W["prefetch"] and i + 1 < len(stacks):
            readahead(stacks[i + 1])
        out.append(_one(st))
    after = J2KDecoder.stats()
    return out, {k: after[k] - before[k] for k in after}


def _serial(model, stacks, decode):
    with one_thread():
        for st in stacks:
            try:
                r = fusion.predict(model, st.files, st.extra.get("header"), decode), None
            except Exception as e:  # noqa: BLE001
                r = None, outcome(e)
            yield st, *r


def score(
    model: fusion.Model,
    inputs: Path,
    stacks: list[manifest.Stack],
    workers: int = 1,
    device: str = "cpu",
    encoder_device: str = "cpu",
    batch: int = 64,
    chunk: int = 4,
    prefetch: bool | None = None,
    stats: dict | None = None,
    gpu_workers: int = 2,
) -> Iterator[tuple[manifest.Stack, fusion.StackResult | None, tuple[str, str] | None]]:
    """Every stack's answer, in the manifest's order."""
    if prefetch is None:
        prefetch = os.environ.get("NILS_BODYPART_PREFETCH", "1") != "0"
    gpu_decode = device == "cuda"
    torch_encoder = None
    if encoder_device == "cuda":
        from .gpu import TorchEncoder

        torch_encoder = TorchEncoder(model.encoder)
    if workers <= 1 and torch_encoder is None:
        decode = None
        if gpu_decode:
            try:
                from .gpu import J2KDecoder

                decode = J2KDecoder()
            except Exception as e:  # noqa: BLE001
                logger.warning("the GPU decoder did not start (%s); decoding on the CPU", type(e).__name__)
        yield from _serial(model, stacks, decode)
        if stats is not None and decode is not None:
            stats.update(decode.stats())
        return

    # the workers are spawned as they are needed, so the environment they
    # start with stays set until the pool is done
    saved = {k: os.environ.get(k) for k in ONE_THREAD}
    os.environ.update(ONE_THREAD)
    try:
        yield from _pooled(model, inputs, stacks, workers, gpu_decode, torch_encoder, batch, chunk, prefetch, stats, gpu_workers)
    finally:
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def _pooled(model, inputs, stacks, workers, gpu_decode, torch_encoder, batch, chunk, prefetch, stats, gpu_workers):
    ctx = mp.get_context("spawn")
    pool = ProcessPoolExecutor(
        max_workers=max(1, workers),
        mp_context=ctx,
        initializer=_init,
        initargs=(str(inputs), gpu_decode, torch_encoder is not None, prefetch, ctx.Value("i", 0), gpu_workers),
    )
    chunks = [stacks[i : i + chunk] for i in range(0, len(stacks), chunk)]
    window = max(2, workers) * 2
    pending: deque = deque()
    held: list = []  # prepared stacks waiting for the batched encoder
    decoded = {"gpu_frames": 0, "cpu_frames": 0}

    def flush():
        if not held:
            return
        P = torch_encoder.probabilities([p.built.vol for _, p in held], [p.built.geo() for _, p in held])
        for (st, prep), p in zip(held, P):
            yield st, fusion.finish(model, prep, p), None
        held.clear()

    with pool:
        it = iter(chunks)
        for c in it:
            pending.append((c, pool.submit(_chunk, c)))
            if len(pending) >= window:
                break
        while pending:
            c, fut = pending.popleft()
            nxt = next(it, None)
            if nxt is not None:
                pending.append((nxt, pool.submit(_chunk, nxt)))
            got, st_stats = fut.result()
            for k in decoded:
                decoded[k] += st_stats.get(k, 0)
            for st, (r, why) in zip(c, got):
                if r is None:
                    yield from flush()
                    yield st, None, why
                elif torch_encoder is not None:
                    held.append((st, r))
                    if len(held) >= batch:
                        yield from flush()
                else:
                    yield st, r, None
        yield from flush()
    if stats is not None and gpu_decode:
        stats.update(decoded)
