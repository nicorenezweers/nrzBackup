# nrzBackup image for umbrelOS (linux/amd64 + linux/arm64).
# The server binaries and the web app are built beforehand on the host
# (scripts/release.sh: cargo-zigbuild + dx), this file only packs them – no
# compiler, no emulation, small image.
FROM debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4

ARG TARGETARCH
COPY --chmod=755 dist/nrzbackup-server-${TARGETARCH} /usr/local/bin/nrzbackup-server
COPY dist/public /app/public

ENV NRZBACKUP_ADDR=0.0.0.0:8080 \
    NRZBACKUP_DATA=/data \
    NRZBACKUP_PUBLIC=/app/public

# umbrelOS creates app directories as 1000:1000
USER 1000:1000
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["nrzbackup-server", "healthcheck"]
CMD ["nrzbackup-server"]
