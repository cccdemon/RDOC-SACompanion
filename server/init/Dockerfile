# InitConnection signaling server. Build context = repo root.
FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p init-connection

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/init-connection /usr/local/bin/init-connection
EXPOSE 8080
ENTRYPOINT ["init-connection"]
