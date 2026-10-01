FROM rust:1.85-bookworm@sha256:e51d0265072d2d9d5d320f6a44dde6b9ef13653b035098febd68cce8fa7c0bc4 AS build

WORKDIR /src
COPY . .
RUN cargo build --release -p lxcup-agent

FROM scratch AS artifact
ARG TARGETARCH
COPY --from=build /src/target/release/lxcup-agent /linux-${TARGETARCH}
