use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::args::GenerateDockerArgs;
use crate::commands::{EXIT_INVALID_ARGS, EXIT_OK};

const FILES: [(&str, &str); 3] = [
    (
        "Dockerfile",
        r#"# Build from the repository root: docker build -f crates/sentinel-cli/Dockerfile .
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release -p sentinel-cli --bin sentinel

FROM alpine:3.21
RUN apk add --no-cache ca-certificates \
    && addgroup -S -g 10001 sentinel \
    && adduser -S -D -H -u 10001 -G sentinel sentinel
COPY --from=builder /src/target/release/sentinel /usr/local/bin/sentinel
USER 10001:10001
ENV HOME=/tmp
ENTRYPOINT ["/usr/local/bin/sentinel"]
CMD ["check", "--config", "/etc/sentinel/sentinel.toml"]
"#,
    ),
    (
        "docker-compose.yml",
        r#"services:
  sentinel:
    build:
      context: .
      dockerfile: crates/sentinel-cli/Dockerfile
    image: soroban-sentinel:local
    restart: unless-stopped
    read_only: true
    user: "10001:10001"
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
    tmpfs:
      - /tmp:rw,noexec,nosuid,nodev,size=16m,uid=10001,gid=10001
    environment:
      SOROBAN_RPC_URL: ${SOROBAN_RPC_URL:-}
      RUST_LOG: ${RUST_LOG:-info}
    volumes:
      - ./sentinel.toml:/etc/sentinel/sentinel.toml:ro
    healthcheck:
      test: ["CMD", "/usr/local/bin/sentinel", "doctor", "--network", "${SENTINEL_NETWORK:-testnet}"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 20s
"#,
    ),
    (
        ".dockerignore",
        r#".git
.github
target
**/target
.env
.env.*
!.env.example
**/*.pem
**/*.key
**/*secret*
**/*credentials*
"#,
    ),
];

pub fn run_generate_docker(args: GenerateDockerArgs) -> i32 {
    if !args.out_dir.is_dir() {
        eprintln!(
            "Error: output directory '{}' does not exist",
            args.out_dir.display()
        );
        return EXIT_INVALID_ARGS;
    }

    let existing = FILES
        .iter()
        .map(|(name, _)| args.out_dir.join(name))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if !existing.is_empty() {
        eprintln!(
            "Error: output file '{}' already exists; move it before generating Docker files",
            existing[0].display()
        );
        return EXIT_INVALID_ARGS;
    }

    for (name, contents) in FILES {
        let path = args.out_dir.join(name);
        if let Err(error) = write_new_file(&path, contents) {
            eprintln!("Error: could not write '{}': {error}", path.display());
            return EXIT_INVALID_ARGS;
        }
    }

    println!(
        "Wrote Docker deployment files to {}",
        args.out_dir.display()
    );
    EXIT_OK
}

fn write_new_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents.as_bytes())?;
    file.flush()?;
    // Ensure restrictive defaults for generated deployment files (subject to umask).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}
