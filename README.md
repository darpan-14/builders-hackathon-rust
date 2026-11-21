# Shopping Sessions in Rust

A Rust implementation of the Shopping Sessions buildathon track using embedded DuckDB 1.5.6. The executable reads the mounted CSV files and manifest, then writes Parquet to `user_daily/` and `campaign_daily/`. The C++ implementation is in a separate repository. [Contest materials](https://www.joinbuilders.org/buildathon/tracks/shopping-sessions) · [C++ repository](https://github.com/darpan-14/builders-hackathon)

## Build and run

For a native build, use Rust 1.92 or newer, curl, and unzip. Fetch the pinned DuckDB library once, then build with the checked-in Cargo lockfile.

```sh
scripts/fetch-duckdb.sh .local/duckdb
cargo test --locked
cargo build --release --locked
NCPU=8 target/release/shopping-sessions /path/to/input /path/to/empty-output
```

The root Dockerfile builds the submission image. The image accepts the scorer's `DATA_DIR`, `OUTPUT_DIR`, `SCRATCH_DIR`, `NCPU`, and `MEM_GB` settings. `SHOP_PROFILE=1` prints elapsed time for each processing stage.

```sh
docker buildx build --platform linux/amd64 --load -t shopping-sessions:rust .
IMAGE=shopping-sessions:rust scripts/docker_run.sh /path/to/input /path/to/empty-output
```

## Implementation

Rust workers scan newline-aligned blocks of memory-mapped input. The CSV scanner finds delimiters and validates quoting in batches. Unusual or malformed blocks go through a scalar parser implementing the same dialect.

A first pass indexes event IDs in partitioned bitmaps. Any ID that occurs twice is guaranteed to reach the exact deduplication table. There, the earliest ingest timestamp wins even if its payload is invalid. Click attribution uses a sliding seven-day window and counts each active campaign once per session.

Union-find assigns every linked alias the byte-wise smallest name of its component. Workers group events and clicks by numeric person ID and process each person's timeline. DuckDB reads the daily rows through parallel table functions, combines campaign subtotals with SQL, and writes both Parquet tables.

## Check the results

Extract the organizers' DuckDB starter and dev answer, and install Python with Polars. Run the official fixtures and compare the seed-1 dev output with its reference.

```sh
python3 scripts/run_fixtures.py /path/to/starter-duckdb/fixtures -- target/release/shopping-sessions
python3 scripts/compare.py /path/to/dev-output /path/to/dev-answer
```

The manually triggered x64 benchmark builds both production images, generates the same practice and final-size data for each, alternates execution order, and compares both tables. The workflow pins the C++ revision and records the hardware and run times. [Benchmark workflow](.github/workflows/benchmark.yml)

## License and credits

The Rust solution is released under the MIT license. The organizers supplied the generator, fixtures and reference answers. The local container wrapper and fixture checker come from their DuckDB starter. The DuckDB download script and output comparison tools are shared with the C++ repository. [License](LICENSE)
