FROM arm64v8/rust@sha256:1dc7c15e251ba960de736b8940eb17a2ff41d81bde13a47cf964102621c57d6d

RUN apt update && apt upgrade -y
RUN apt install -y libdbus-1-dev pkg-config

WORKDIR /build

COPY . .

ENV RUSTFLAGS="-C debuginfo=1"

CMD ["cargo", "build", "--release", "--target", "aarch64-unknown-linux-gnu"]
