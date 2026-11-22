# syntax=docker/dockerfile:1
#
# oa-server 生产镜像。
# 客户端（桌面 / 移动 / 浏览器）只是 REST 客户端，服务端是唯一数据源。
# 数据库由独立的 Postgres 容器（或外部实例）提供，本镜像只承载服务端进程与附件目录。
#
# 构建并运行：docker compose up -d --build
# 单独构建：  docker build -t oa-server:0.1.0 .
# 访问：      浏览器打开 http://localhost:8787 （前后端同源，API 走相对路径 /api）
#
# 用 rust:1-bookworm 而非 alpine：oa-core 走 native-tls（系统 OpenSSL），
# musl 目标需要另装 openssl-dev 且链接行为不同，debian 这条路径更接近本机验证环境。

# ---- 前端构建：容器内 npm ci + Vite，API 地址留空走同源相对路径 ----
FROM node:22-bookworm-slim AS web-build
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
# 网络抖动时自动重试；registry 可用 npm_config_registry 覆盖
RUN npm ci --fetch-retries=5 --fetch-retry-maxtimeout=120000
COPY frontend/ ./
ENV VITE_API_BASE="" VITE_API_LAN="" VITE_API_WAN=""
RUN npm run build

FROM rust:1-bookworm AS build
WORKDIR /src

# 先只拷清单，让依赖解析层尽可能命中缓存；源码放在后面
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# src-tauri 也是 workspace 成员：它的 [lib] name = "oa_app_lib" 要求 cargo 能看到 src/lib.rs，
# 否则连 `-p oa-server` 都会在加载 workspace 时失败（can't find library `oa_app_lib`）。
# 只带清单与源码，不装 webkit2gtk 等桌面依赖，oa-app 本身不参与编译。
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
COPY src-tauri/build.rs ./src-tauri/build.rs
COPY src-tauri/src ./src-tauri/src

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
# 前端静态资源：与 API 同源，由 oa-server 的 ServeDir fallback 托管
COPY --from=web-build /web/dist /usr/share/oa/web

# 附件目录必须落在卷上，容器重建不丢文件；数据库在 Postgres 容器中持久化
RUN useradd --system --uid 10001 --create-home oa && \
    mkdir -p /data/oa-files && \
    chown -R oa:oa /data

USER oa
WORKDIR /data

# OA_DB 由 compose / 部署环境显式提供，镜像不内置任何数据库地址
ENV OA_ADDR=0.0.0.0:8787 \
    OA_FILES=/data/oa-files \
    OA_WEB_ROOT=/usr/share/oa/web

EXPOSE 8787
VOLUME ["/data/oa-files"]

# 起始等待给足 10s：首次启动要建表 + 写种子数据
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${OA_ADDR##*:}/api/health" || exit 1

ENTRYPOINT ["/usr/local/bin/oa-server"]
