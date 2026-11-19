# Build and test image for the Rust server.
#
# This is the ONLY step that touches the network. Everything it installs comes
# from Ubuntu's signed apt archive: the compiler and (later) every Rust crate
# (librust-*-dev). Nothing is fetched from crates.io or GitHub, here or later.
# Code is then compiled by tools/dev.sh with networking disabled.
#
# Crates are admitted by owner decision (docs/reference/dependencies.md). Each
# librust-*-dev line below pairs with rows in server/deps.allow and a digest
# in server/deps.digest. The first is getrandom (OS randomness); apt pulls in
# the crates it names (libc, cfg-if and the wasm-only ones). python3 is for
# `tools/check_deps.py --registry` and `tools/dep-notices.py`, which read the
# registry (stdlib only; nothing is installed with pip).
#
# The base image is pinned by digest; bump it deliberately.
FROM ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates gcc libc6-dev \
      rustc cargo rust-clippy rustfmt \
      librust-getrandom-dev \
      python3 \
 && rm -rf /var/lib/apt/lists/*

# Record exactly which crates the image carries, for review in a diff.
RUN ls /usr/share/cargo/registry | sort > /etc/eve-offline-crates.txt

# Record the toolchain too: apt's rustc/cargo are not version-pinned, so a
# rebuild can move them. The gate's `== toolchain` step prints this (V6 F12).
RUN { rustc -V; cargo -V; } > /etc/eve-offline-toolchain.txt

WORKDIR /repo/server
