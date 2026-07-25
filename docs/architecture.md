# `bootart` Architecture Specification

## Invariant

> `bootart` renders a finite sequence of terminal frames and exits. It never owns boot state.

```text
launcher → inherited TTY → renderer → exit
```

`bootart` is designed as a standalone, non-daemonized, single-pass ASCII animation renderer for early Linux boot.

## Design Goals

- **Zero Daemons / Server Architecture**: No sockets, no background processes, no IPC, no D-Bus, no Plymouth compatibility layer.
- **Self-Contained**: Statically linked musl binary with embedded ASCII artwork (`assets/logo.txt`).
- **Foreground Execution**: Runs for ~900 ms, renders its frames directly to the inherited standard output (`/dev/tty1`), and exits cleanly.
- **Fail-Safe**: Any failure or signal immediately restores terminal cursor and attributes and exits without stopping the Linux boot sequence.

## Component Breakdown

1. `main.rs` & `cli.rs`: Command line parsing for `play`, `render-final`, `validate`, `preview`.
2. `art.rs`: Art file validation, line trimming, and layout calculation (`layout(art_size, term_size)`).
3. `animation.rs`: Deterministic SplitMix64 integer hash, diagonal reveal threshold computation, and color wave progress.
4. `renderer.rs`: In-memory ANSI frame construction, single-write frame flushing, color code coalescing, and cursor management.
5. `terminal.rs`: `TerminalOutput` trait abstracting `StdoutTerminal` (`libc::ioctl(STDOUT_FILENO, TIOCGWINSZ, ...)`) and test `BufferTerminal`.
6. `signals.rs`: Signal handling using `libc::sigaction` for `SIGTERM`, `SIGINT`, `SIGHUP` setting an `AtomicBool` flag.
