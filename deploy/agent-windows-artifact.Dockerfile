FROM rust:1.85-bookworm@sha256:e51d0265072d2d9d5d320f6a44dde6b9ef13653b035098febd68cce8fa7c0bc4 AS build

WORKDIR /src
COPY . .

RUN apt-get update \
    && apt-get install --yes --no-install-recommends gcc-mingw-w64-x86-64 \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-pc-windows-gnu

ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
RUN cargo build --locked --release -p lxcup-agent --target x86_64-pc-windows-gnu

FROM scratch AS artifact
COPY --from=build /src/target/x86_64-pc-windows-gnu/release/lxcup-agent.exe /windows-amd64.exe
