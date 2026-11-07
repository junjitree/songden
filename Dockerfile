# Multi-stage build of the single `songden` binary. reqwest uses rustls, so the
# runtime needs CA certificates and nothing else.
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY static static
RUN cargo build --release --locked && cp target/release/songden /usr/local/bin/songden

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 songden \
    && mkdir /state && chown songden /state
COPY --from=builder /usr/local/bin/songden /usr/local/bin/songden
USER songden
ENV RUST_LOG=info STATE_DIR=/state
VOLUME /state
EXPOSE 8090
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/songden"]
