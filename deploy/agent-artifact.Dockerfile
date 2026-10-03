FROM --platform=$BUILDPLATFORM rust:1.85-bookworm@sha256:e51d0265072d2d9d5d320f6a44dde6b9ef13653b035098febd68cce8fa7c0bc4 AS build

ARG TARGETARCH

RUN apt-get update \
    && apt-get install --yes --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN rustup target add aarch64-unknown-linux-gnu --toolchain 1.85.0
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        cargo build --locked --release -p lxcup-agent --target aarch64-unknown-linux-gnu \
        && cp target/aarch64-unknown-linux-gnu/release/lxcup-agent /src/lxcup-agent-artifact; \
    else \
        cargo build --locked --release -p lxcup-agent \
        && cp target/release/lxcup-agent /src/lxcup-agent-artifact; \
    fi

FROM scratch AS artifact
ARG TARGETARCH
COPY --from=build /src/lxcup-agent-artifact /linux-${TARGETARCH}
