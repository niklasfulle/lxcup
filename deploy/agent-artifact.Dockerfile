FROM rust:1.85-bookworm AS build

WORKDIR /src
COPY . .
RUN cargo build --release -p lxcup-agent

FROM scratch AS artifact
COPY --from=build /src/target/release/lxcup-agent /linux-amd64
