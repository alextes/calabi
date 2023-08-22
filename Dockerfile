# Use the official Rust image to build the binary
FROM rust as builder
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/
RUN echo "fn main() { }" > src/main.rs
RUN cargo build --release
COPY src ./src
RUN cargo build --release

# Use the minimal Alpine image to run the binary
FROM alpine:latest
RUN apk --no-cache add ca-certificates
COPY --from=builder /usr/src/app/target/release/calabi /usr/local/bin/calabi
CMD ["calabi"]
