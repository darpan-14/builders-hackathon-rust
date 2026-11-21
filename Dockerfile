FROM rust:1.92-slim-bookworm AS build
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates unzip && rm -rf /var/lib/apt/lists/*
COPY scripts/fetch-duckdb.sh scripts/
RUN scripts/fetch-duckdb.sh .local/duckdb
COPY Cargo.toml Cargo.lock build.rs ./
COPY src src
RUN if [ "$(uname -m)" = x86_64 ]; then export RUSTFLAGS="-Ctarget-cpu=x86-64-v3"; fi && cargo build --locked --release

FROM debian:bookworm-slim
COPY --from=build /app/.local/duckdb/libduckdb.so /usr/local/lib/
RUN ldconfig
COPY --from=build /app/target/release/shopping-sessions /usr/local/bin/
ENV DATA_DIR=/data OUTPUT_DIR=/output SCRATCH_DIR=/scratch HOME=/scratch NCPU=8 MEM_GB=24
USER 1000:1000
CMD ["shopping-sessions"]
