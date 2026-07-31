FROM rust:1.97-bookworm AS builder

WORKDIR /source
COPY Cargo.toml Cargo.lock build.rs rust-toolchain.toml ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN groupadd --system converter \
    && useradd --system --gid converter --home-dir /app converter \
    && mkdir -p /app /data/spool /data/output \
    && chown -R converter:converter /data /app

COPY --from=builder /source/target/release/base64-file-converter /app/base64-file-converter

USER converter
WORKDIR /app
VOLUME ["/data/spool", "/data/output"]
ENTRYPOINT ["/app/base64-file-converter"]
CMD ["capabilities"]
