# syntax=docker/dockerfile:1
#
# oa-server 生产镜像。
# 客户端（桌面 / 移动 / 浏览器）只是 REST 客户端，服务端是唯一数据源，
# 所以对外发布真正要部署的就是这个进程 + 一个持久化数据目录。
#
# 构建：docker build -t oa-server:0.1.0 .
# 运行：docker run -d -p 8787:8787 -v oa-data:/data --name oa-server oa-server:0.1.0
#
# 用 rust:1-bookworm 而非 alpine：oa-core 走 native-tls（系统 OpenSSL），
# musl 目标需要另装 openssl-dev 且链接行为不同，debian 这条路径更接近本机验证环境。

FROM rust:1-bookworm AS build
WORKDIR /src

# 先只拷清单，让依赖解析层尽可能命中缓存；源码放在后面
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
COPY src-tauri/build.rs ./src-tauri/build.rs

# 只构建 oa-server：workspace 里的 oa-app（Tauri）不会被编译，也就不需要 webkit2gtk 等系统库
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --package oa-server && \
    cp /src/target/release/oa-server /usr/local/bin/oa-server

FROM debian:bookworm-slim AS runtime

# libssl3：native-tls 的运行时依赖；ca-certificates：出网调 LLM/HTTPS；
# curl：仅用于 HEALTHCHECK（debian-slim 没有自带探活工具）
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/oa-server /usr/local/bin/oa-server

# 数据目录（oa.db + oa-files/）必须落在卷上，容器重建不丢数据
RUN useradd --system --uid 10001 --create-home oa && \
    mkdir -p /data/oa-files && \
    chown -R oa:oa /data

USER oa
WORKDIR /data

ENV OA_ADDR=0.0.0.0:8787 \
    OA_DB=/data/oa.db \
    OA_FILES=/data/oa-files

EXPOSE 8787
VOLUME ["/data"]

# 起始等待给足 10s：首次启动要建库 + 写种子数据
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${OA_ADDR##*:}/api/health" || exit 1

ENTRYPOINT ["/usr/local/bin/oa-server"]
