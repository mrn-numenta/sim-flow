# syntax=docker/dockerfile:1.6
#
# CI runner image for sim-flow.
#
# Pre-installs the Rust toolchain, cargo-llvm-cov, and the system tools the
# workflow runs ad-hoc (jq, git). Built and pushed by
# .github/workflows/build-ci-image.yml.

FROM rust:1.95-slim-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        git \
        jq \
        libssl-dev \
        openssh-client \
        pkg-config \
        protobuf-compiler \
 && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy llvm-tools-preview

ARG LLVM_COV_VERSION=0.8.5
RUN cargo install cargo-llvm-cov --locked --version "${LLVM_COV_VERSION}"

# Trailing apt layer for system libs the crate links against:
#   - libfontconfig1-dev for plotters / font-kit, pulled in transitively
#     through foundation-framework
# Kept in a trailing layer so toolchain + cargo-llvm-cov layers stay cached.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        libfontconfig1-dev \
 && rm -rf /var/lib/apt/lists/*

# Bake `sim-foundation` into the image as a local mirror clone. Combined
# with the `insteadOf` rewrite below, this lets cargo fetch the private
# foundation-framework + block-diagram deps at CI time without any network
# credentials — critical for PRs from forks, which by GitHub policy cannot
# read repository secrets (so FOUNDATION_DEPLOY_KEY is unavailable in that
# context).
#
# Confidentiality of the baked mirror: sim-flow is private, so forking it
# requires read access to the NumentaCorp org, and that access also covers
# sim-foundation. Anyone who can open a fork PR against this repo could
# already `git clone` foundation directly — baking the mirror changes the
# fetch path, not the access boundary. Revisit if sim-flow ever becomes
# public, or if outside collaborators are granted fork-PR access without
# sim-foundation access.
#
# The deploy key is consumed only as a BuildKit secret. It is written to
# /root/.ssh/id_ed25519 and `rm`'d back out before this RUN ends, so the
# state with the key on disk is never committed into a layer — there is no
# layer it could leak from. Pointing `GIT_SSH_COMMAND` at /run/secrets/...
# directly would also work, but the key is confined to the build container
# either way; this isn't a security difference, just a different placement
# of the same exposure window.
#
# The github.com ed25519 host key is pinned inline rather than via
# ssh-keyscan (TOFU/MITM-resistant). Source:
# https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints
RUN --mount=type=secret,id=foundation_deploy_key,mode=0400,uid=0,gid=0 \
    set -eu \
 && mkdir -p /root/.ssh \
 && printf '%s\n' "$(cat /run/secrets/foundation_deploy_key)" > /root/.ssh/id_ed25519 \
 && chmod 600 /root/.ssh/id_ed25519 \
 && printf '%s\n' 'github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl' > /root/.ssh/known_hosts \
 && git clone --mirror ssh://git@github.com/NumentaCorp/sim-foundation.git /opt/sim-foundation.git \
 && rm -rf /root/.ssh

# Rewrite the foundation SSH URL to the local mirror at fetch time. Cargo
# still hashes the original ssh://... URL when computing its source-id, so
# Cargo.lock entries pinning `git+ssh://...#<sha>` resolve unchanged — only
# the underlying transport switches from SSH to local file.
RUN git config --system url."file:///opt/sim-foundation.git/".insteadOf "ssh://git@github.com/NumentaCorp/sim-foundation.git"

# Route cargo's git ops through the system `git` binary so the `insteadOf`
# rewrite above is honored. (libgit2 ignores git's config files.) Set in the
# image so workflow steps don't have to repeat it.
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /workspace
