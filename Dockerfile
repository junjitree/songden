# Multi-stage build of the single `songden` binary, plus the tools it runs to
# fetch songs: yt-dlp, Deno (yt-dlp needs a JavaScript runtime for YouTube) and
# ffmpeg (to tidy the downloaded audio into a plain .m4a).
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY static static
RUN cargo build --release --locked && cp target/release/songden /usr/local/bin/songden

FROM python:3.13-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 1000 songden \
    && python -m venv /opt/ytdlp \
    && /opt/ytdlp/bin/pip install --no-cache-dir "yt-dlp[default]" deno \
    && chown -R songden /opt/ytdlp \
    && mkdir /state /music && chown songden /state /music
COPY --from=builder /usr/local/bin/songden /usr/local/bin/songden
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
USER songden
ENV PATH="/opt/ytdlp/bin:$PATH" RUST_LOG=info STATE_DIR=/state MUSIC_DIR=/music
VOLUME ["/state", "/music"]
EXPOSE 8090
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
