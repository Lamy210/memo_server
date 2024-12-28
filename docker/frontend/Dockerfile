FROM node:20-alpine

WORKDIR /app

# 必要なパッケージのインストール
RUN apk add --no-cache git bash

# グローバルパッケージのインストール
RUN npm install -g pnpm

# シェルをbashに変更
SHELL ["/bin/bash", "-c"]

# コンテナ起動時のデフォルトシェルを設定
ENTRYPOINT ["/bin/bash"]