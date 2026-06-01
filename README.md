# mymon

A lean Linux system monitor with a custom braille-based TUI. One compact
overview block (per-core CPU, memory, per-GPU utilization and per-GPU
memory) plus a side-by-side process list sorted by CPU and by resident
memory.

## Sample render

```
┌ devbox · 5950X · 64.0G · A6000x4 · 423 procs · up 2d 14:23:11 ───────────────────────────────────────────────────────┐
│CPU  47% ⡄          ⣀  ⡄                                         ⢀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⡀                                │
│MEM  59% ⣿⣦⡀      ⣤ ⣿⡇ ⣿  ⣀⣀⣀⣀⣀⣠⣤⣤⣤⣤⣤⣤⣤⣤⣴⣒⣒⣒⣒⣒⣒⣒⣒⣒⣒⣋⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣀⣀⣀                ⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⣉⣉⣉⣉⣉⣓⣒⣒⣒⣒⣒⣒⣒⣒⣒⣦⣤⣤⣤⣤⣤⣤⣤│
│GPU  65% ⣿⣿⣿⣦⣀⡀   ⣿ ⣿⣧ ⣿⣆            ⢀⣀⣀⣀⣀⡤⠤⠤⠴⠒⠒⠒⠋⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠙⠛⠛⠛⠻⠭⠭⠭⠭⣍⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⣉⡭⠭⠭⠭⠽⠛⠛⠛⠋⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉│
│VRM  54% ⣿⣿⣿⣿⣿⣿⣷⣶ ⣿ ⣿⣿ ⣿⣿ ⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉                                                                                │
└───────────────────────────▴─────────────────────────────▴─────────────────────────────▴──────────────────────────────┘
┌    PID USER      CPU%   MEM  VRAM COMMAND────────────────┐┌    PID USER      CPU%   MEM  VRAM COMMAND────────────────┐
│   1000 alice    312.4 18.2G     - train.py               ││   1000 alice    312.4 18.2G     - train.py               │
│   1137 alice    198.7  4.6G     - rustc                  ││   1137 alice    198.7  4.6G     - rustc                  │
│   1274 alice     96.3  1.1G     - cargo                  ││   1548 alice     22.1  3.8G     - firefox                │
│   1411 bob       47.8  2.4G     - clangd                 ││   1411 bob       47.8  2.4G     - clangd                 │
│   1548 alice     22.1  3.8G     - firefox                ││   1274 alice     96.3  1.1G     - cargo                  │
│   1685 alice      0.4 12.3M     - zsh                    ││   2096 root       0.1 20.5M     - systemd                │
│   1822 bob        0.3 15.4M     - vim                    ││   1822 bob        0.3 15.4M     - vim                    │
│   1959 alice      0.1  8.2M     - tmux                   ││   1685 alice      0.4 12.3M     - zsh                    │
│   2096 root       0.1 20.5M     - systemd                ││   2233 root       0.0  9.2M     - dbus-daemon            │
│   2233 root       0.0  9.2M     - dbus-daemon            ││   1959 alice      0.1  8.2M     - tmux                   │
└──────────────────────────────────────────────────────────┘└──────────────────────────────────────────────────────────┘
```

In a real terminal the gutter readouts, bar columns and history lines
each carry their own color (CPU cyan, MEM blue, GPU magenta, GPU memory
light magenta); bar cells that climb into the upper bands flash yellow
above 50%, orange above 75%, and red above 90%. The `M`/`G` suffixes in
the memory columns are dimmed so the numbers read first. White ▴ marks
on the bottom frame of the overview are one-minute time ticks counted
back from "now" at the right edge of the graph. The left process pane
is sorted by CPU, the right by resident memory.

The CPU and GPU model numbers in the title are extracted by digit
density (so `13th Gen Intel(R) Core(TM) i7-1370P` collapses to
`i7-1370P` and `NVIDIA RTX A6000` collapses to `A6000`). Repeated GPUs
are folded into `A6000x4`.

When the process area is narrower than 110 columns the dual pane
collapses to a single CPU-sorted pane.

## Build and run

```sh
cargo run --release
```

Press `q`, `Esc`, or `Ctrl-C` to quit.

GPU monitoring requires NVIDIA NVML (the `libnvidia-ml` library that
ships with the proprietary driver). Without it, GPU rows simply read
`--`.

## Configuration

Two env vars override the refresh cadences (milliseconds), useful on
big machines where the `/proc` scan is pricey:

| Variable | Default | Drives |
| --- | --- | --- |
| `MYMON_INTERVAL_MS` | `1000` | CPU / memory / GPU stats and the history graph |
| `MYMON_PROC_INTERVAL_MS` | `2000` | The expensive process-table scan |
