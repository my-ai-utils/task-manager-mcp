#!/usr/bin/env bash
# Builds the browser client (in ui/) and copies it into wwwroot/, which is committed and baked into the
# docker image by the Dockerfile. Run this after changing anything under ui/ or shared/, then commit the
# updated wwwroot/. Just a convenience wrapper so you do not have to cd into ui/ first.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

exec "${SCRIPT_DIR}/ui/build.sh"
