# 前端构建阶段
FROM node:22-alpine AS frontend-builder

WORKDIR /app/frontend
COPY frontend/package.json frontend/pnpm-lock.yaml* ./
RUN npm install -g pnpm && pnpm install
COPY frontend ./
RUN pnpm build

# 后端构建阶段
FROM rust:1.84-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static

WORKDIR /app
COPY backend/Cargo.toml backend/Cargo.lock* ./
COPY backend/src ./src
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

RUN cargo build --release

# 运行阶段
FROM alpine:3.21

RUN apk add --no-cache ca-certificates

WORKDIR /app
COPY --from=builder /app/target/release/kiro-rs /app/kiro-rs

VOLUME ["/app/data"]

EXPOSE 8990

CMD ["./kiro-rs", "-c", "/app/data/config.toml"]
