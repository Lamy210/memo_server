FROM rust:1.81-slim-bullseye

WORKDIR /app

# 開発に必要なパッケージのインストール
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    git \
    curl \
    && rm -rf /var/lib/apt/lists/*

# cargo-watchのインストール（開発用）
RUN cargo install cargo-watch

# コンテナ起動時のデフォルトコマンド
CMD ["/bin/bash"]