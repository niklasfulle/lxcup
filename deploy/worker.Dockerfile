FROM rust:1.85-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p lxcup-ansible-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ansible-core ca-certificates curl openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 lxcup
COPY --from=build /src/target/release/lxcup-ansible-worker /usr/local/bin/lxcup-ansible-worker
USER lxcup
ENTRYPOINT ["/usr/local/bin/lxcup-ansible-worker"]
