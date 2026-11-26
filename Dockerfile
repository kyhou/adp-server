FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p adp-server --bin adp-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 adp \
 && install -d -o adp -g "$(id -gn adp)" /data
COPY --from=build /src/target/release/adp-server /usr/local/bin/adp-server
USER adp
WORKDIR /data
ENV ADP_BIND_ADDR=0.0.0.0:8080 \
    ADP_DB_PATH=/data/adp.db \
    ADP_FILES_DIR=/data/files
EXPOSE 8080
ENTRYPOINT ["adp-server"]
