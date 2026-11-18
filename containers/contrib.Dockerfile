# A contributor's host for tools/export-parity.sh: python 3.11 (the floor), standard
# library only, in /usr/local/bin rather than /usr/bin; git, bash, coreutils. No protobuf,
# no client, no private/. Same pinned base as analysis.Dockerfile (python:3.11-slim).
FROM python@sha256:e88e9763f943ec1834f992a4b51e0f24500486803e8bc534e5767af9ea65f6ce
RUN apt-get update && apt-get install -y --no-install-recommends git \
 && rm -rf /var/lib/apt/lists/*
# The hostile git config is not baked in: tools/export-parity.sh mounts containers/contrib-gitconfig
# from the export at /etc/contrib-gitconfig, read-only, so the image cannot drift from the file.
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 LANG=C.UTF-8
