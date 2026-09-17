FROM node:24-alpine

WORKDIR /app

RUN apk add --no-cache bash curl \
    && corepack enable \
    && corepack prepare pnpm@9.15.9 --activate

CMD ["pnpm", "dev", "--host", "0.0.0.0", "--port", "3000"]
