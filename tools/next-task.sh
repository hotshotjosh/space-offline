#!/usr/bin/env bash
# Open issues, `status/ready` first: a read-only sorting hint. Nothing it prints grants anything.
# Flags: --no-client, --area SLUG, --limit N. See tools/tracker.py issues --help.
exec python3 -I "$(dirname "$0")/tracker.py" issues "$@"
