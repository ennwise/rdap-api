# Stage 1: Build
FROM rust:latest AS builder

# Set the working directory
WORKDIR /usr/src/app

# Copy the Cargo.toml and Cargo.lock files
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs file to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build the dependencies
RUN cargo build --release && rm -rf src

# Copy the source code
COPY . .

# Build the project
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install necessary dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 50000 appgroup && useradd -u 50000 -g appgroup appuser

# Set the working directory
WORKDIR /usr/src/app

# Copy the built binary from the builder stage
COPY --from=builder /usr/src/app/target/release/rdap-api .

# Change ownership of the files to user 50000 and group 50000
RUN chown -R appuser:appgroup /usr/src/app

# Expose the port that the application will run on
EXPOSE 3030

# Switch to the new user
USER appuser

# Set the entrypoint to the binary
CMD ["./rdap-api"]