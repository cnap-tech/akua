# Minimal akua container — just the static-ish CLI binary, no helm/kubectl.
# Consumers layer their own tooling on top (kubectl, helm, argocd CLI, etc).
# Multi-arch: amd64 + arm64. Build context contains per-arch subdirs
# populated by cli-release.yml; `$TARGETARCH` picks the right one.

FROM gcr.io/distroless/cc-debian12:nonroot

# Docker's automatic build arg — "amd64" or "arm64" for the target platform.
# See https://docs.docker.com/build/building/multi-platform/#automatic-platform-args-in-the-global-scope
ARG TARGETARCH

# The binary was stripped at build time (ci-release profile), so a
# distroless runtime is enough. CC variant (not `static`) because
# our rustls build dynamically links libc's unwinder on linux-gnu.
COPY --chmod=0755 ${TARGETARCH}/akua /usr/local/bin/akua

ENTRYPOINT ["/usr/local/bin/akua"]
