# syntax=docker/dockerfile:1
#
# watchtower-rs: musl の静的バイナリ 1 つだけを distroless に載せる。
# x86_64 / aarch64 の両方で同じファイルを使う (docker buildx build --platform linux/amd64,linux/arm64)。
# Alpine の Rust は musl がネイティブなので、どちらのアーキテクチャでも静的リンクになる。

ARG RUST_VERSION=1.99

FROM rust:${RUST_VERSION}-alpine AS build
# C で書かれた依存 (SQLite・zstd・aws-lc・blake3) のビルドに必要
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY bin ./bin
COPY crates ./crates
# 並列度を抑えて、メモリの少ない環境でもビルドできるようにする (CARGO_BUILD_JOBS で変更可)
ARG CARGO_BUILD_JOBS=2
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p watchtower \
    && cp target/release/watchtower /watchtower \
    # データ置き場。distroless には mkdir が無いので、ここで作って所有者を合わせる
    && mkdir /data

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /watchtower /watchtower
COPY --from=build --chown=65532:65532 /data /data

# コンテナの外から届くよう、全インターフェースで待ち受ける。
# 公開するときは必ず HTTPS の背後に置き、ポートの公開先も絞ること (docker-compose.yml を参照)。
ENV WATCHTOWER_DATA_DIR=/data \
    WATCHTOWER_LISTEN=0.0.0.0:5000 \
    WATCHTOWER_LOG_JSON=true
VOLUME /data
EXPOSE 5000

# distroless には curl が無いので、バイナリ自身が /healthz を確かめる
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/watchtower", "healthcheck"]

USER nonroot
ENTRYPOINT ["/watchtower"]
CMD ["run"]
