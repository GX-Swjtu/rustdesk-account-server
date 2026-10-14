ARG RUST_IMAGE=rust:1.99.0-slim-bookworm
FROM ${RUST_IMAGE} AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN groupadd --gid 10001 account && useradd --uid 10001 --gid 10001 --no-create-home account
COPY --from=builder /build/target/release/rustdesk-account-server /usr/local/bin/rustdesk-account-server
USER 10001:10001
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/rustdesk-account-server"]
CMD ["--config", "/etc/account/account.toml", "serve"]

# docker build --target binary --output type=local,dest=dist .
FROM scratch AS binary
COPY --from=builder /build/target/release/rustdesk-account-server /rustdesk-account-server-linux-x86_64

# Keep the runnable image as the default target.
FROM runtime AS final
