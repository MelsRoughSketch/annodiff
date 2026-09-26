# Performance benchmarks

Run from the repository root. Cargo benchmarks use optimized builds:

```sh
cargo bench --locked --bench baseline
cargo bench --locked --bench interactions
cargo bench --locked --bench file_scroll
```

To record a run locally, use `benchmarks/results/`, which is ignored by Git:

```sh
mkdir -p benchmarks/results
cargo bench --locked --bench baseline > benchmarks/results/baseline.csv
cargo bench --locked --bench interactions > benchmarks/results/interactions.csv
cargo bench --locked --bench file_scroll > benchmarks/results/file-scroll.txt
```

## Coverage

| Benchmark | Workload | Measurement |
|---|---|---|
| `baseline` | 100–10,000 lines; unified, side-by-side, wrapping; storage and Git snapshots | 111 cases, median/min/max of three four-operation averages |
| `interactions` | 1,000–100,000 lines; scrolling, selection, divider resizing, filtering, comments | 34 cases with nine individual samples; three additional highlighting-completion measurements |
| `file_scroll` | Sequentially select 12 uncached files of 1,000 or 10,000 lines | Median and maximum of 11 selections |

UI cases include input handling and drawing to Ratatui TestBackend.
`baseline` and `file_scroll` use an 80×25 terminal; `interactions` uses 120×40.
Data setup and syntax-library initialization are excluded.
The fixtures contain generated source code, Unicode, and long lines.

These measurements exclude real terminal output/rendering, input queue latency,
agent communication, and full process startup.
File I/O may benefit from the OS cache.
Timings are not guarantees or pass/fail thresholds.

## Interpreting results

Results are machine- and revision-dependent and are not committed.
Record the commit, toolchain, OS, and CPU when sharing measurements.

For a distant jump into a large diff hunk, `end-cold` measures the first frame,
**not completed syntax highlighting**.
Highlighting yields between short processing slices so input remains responsive.
`end-highlight-complete` separately records full completion (one sample per size);
`end-warm` measures a jump after the relevant code has already been highlighted.
Warm-up for other operations waits for visible highlighting to finish.

## Performance constraints

- Large unparsed hunks can take time to finish highlighting,
  while code remains visible and interactive.
- Changing wrapped column widths recalculates line wrapping across the file,
  reusing allocated buffers.
- Changes to review state save the complete JSON atomically;
  large file sets or extensive history can increase save latency.
- Tracked Git diffs are normally fetched together.
  Untracked files and special diff formats use individual requests.
- Visible rows are rendered on demand;
  derived diff data is cached for the two most recently selected files.
