FROM rust:1.85-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p lxcup-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates wget \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 lxcup
COPY --from=build /src/target/release/lxcup-server /usr/local/bin/lxcup-server
USER lxcup
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/lxcup-server"]
