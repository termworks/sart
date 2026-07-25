# `bootart` Testing Guide

## 1. Unit Tests

Run unit tests via cargo:
```bash
cargo test
```

Unit tests cover:
- ASCII art parsing, CRLF normalization, whitespace trimming, and validation limits.
- Layout math, clipping logic, and small logo selection.
- Deterministic SplitMix64 integer hashing and smoothstep progress curve.
- Terminal abstraction, stdout dimension retrieval, and buffer recording.
- Signal flag atomic updates.

## 2. Golden Frame Tests

Golden frame comparisons ensure deterministic visual rendering across updates:
```bash
cargo test --test golden_tests
```
Golden output files are stored in `tests/golden/frame_*.ans`.

## 3. Pseudo-Terminal (PTY) Tests

PTY tests spawn `bootart` inside an openpty master/slave pair to verify standard output TTY detection, ANSI cursor hiding/showing, and signal clean exit:
```bash
cargo test --test pty_tests
```

## 4. QEMU VM Verification

To test `bootart` in a safe isolated QEMU environment without touching host initramfs:
1. Build static musl binary:
   ```bash
   cargo build --release --target x86_64-unknown-linux-musl
   ```
2. Launch QEMU with a kernel and test initramfs:
   ```bash
   qemu-system-x86_64 -kernel /path/to/vmlinuz -initrd /path/to/initramfs.img -nographic -append "console=ttyS0 quiet"
   ```
