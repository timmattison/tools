# prcp Benchmarks

## Buffer Pool Optimization (Issue #92)

**Date:** 2025-01-14

**Test Setup:**
- Source: 10GB file (`/Volumes/SamsungSSDs/junk.bin`) on NVMe SSD
- Destination: HDD RAID array (`/Volumes/HDDRAID/dest`)
- Transfer type: Cross-device (parallel copy mode)
- Test iterations: 10 runs per configuration

**Results:**

| Configuration | Mean Time | Std Dev | Range |
|--------------|-----------|---------|-------|
| Without buffer pool | 26.073s | ±2.606s | 22.782s - 29.486s |
| With buffer pool | 24.012s | ±1.195s | 23.061s - 26.983s |

**Summary:** Buffer pool is **1.09x faster** (±0.12) than allocating per chunk.

**Raw hyperfine output:**
```
Benchmark 1: prcp /Volumes/SamsungSSDs/junk.bin /Volumes/HDDRAID/dest && rm /Volumes/HDDRAID/dest
  Time (mean ± σ):     26.073 s ±  2.606 s    [User: 30.526 s, System: 3.960 s]
  Range (min … max):   22.782 s … 29.486 s    10 runs

Benchmark 2: prcp --buffer-pool /Volumes/SamsungSSDs/junk.bin /Volumes/HDDRAID/dest && rm /Volumes/HDDRAID/dest
  Time (mean ± σ):     24.012 s ±  1.195 s    [User: 30.713 s, System: 3.581 s]
  Range (min … max):   23.061 s … 26.983 s    10 runs

Summary
  prcp --buffer-pool [...] ran
    1.09 ± 0.12 times faster than prcp [...]
```

**Notes:**
- Buffer pool reuses buffers between reader and writer threads via a return channel
- Pre-allocates `PARALLEL_CHANNEL_DEPTH` (4) buffers at start
- Falls back to allocation if pool is exhausted (reader faster than writer)
- Buffer pool is now enabled by default; use `--no-buffer-pool` to disable
