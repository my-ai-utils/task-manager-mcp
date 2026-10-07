#!/usr/bin/env bash
# Builds the browser client and puts it where the server serves it from.
#
#   ui/  --dx build-->  ui/target/dx/task-manager-ui/release/web/public  --copy-->  wwwroot/
#
# `wwwroot/` is COMMITTED: the release workflow builds only the server and copies that folder into the
# image as it is, so what a release serves is exactly what is in the repository at the tag. Which means
# this script is part of making a release — run it after changing anything under `ui/` or `shared/`, and
# commit the result. A release cut without it ships the previous client with the new server.
#
# The layout and the script are my-no-sql-server's. The one step added is the cache-bust: `build.py`
# stamps a fresh id onto every asset url in `index.html`, so a browser holding last release's wasm is
# sent to fetch this one. `index.html` itself is always revalidated — the server sends it with an ETag.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WWWROOT="${SCRIPT_DIR}/../wwwroot"
DX_OUT="${SCRIPT_DIR}/target/dx/task-manager-ui/release/web/public"

cd "${SCRIPT_DIR}"

# dx does not empty its own output folder between builds, and the wasm and its glue are named by a hash
# of their content — so every build ADDS a pair beside the last one's. Copied as it is, `wwwroot/` would
# carry every client there has ever been: four megabytes more in the repository and in the image per
# release, none of it referenced by `index.html`. Removed here, so the output is exactly one build.
echo ">> cleaning ${DX_OUT}"
rm -rf "${DX_OUT}"

echo ">> dx build --release --web"
dx build --release --web

if [ ! -f "${DX_OUT}/index.html" ]; then
    echo "ERROR: build output not found at ${DX_OUT}"
    exit 1
fi

echo ">> cache-busting ${DX_OUT}/index.html"
python3 build.py "${DX_OUT}/index.html"

# Emptied first, so a file the client no longer ships does not stay in the image for ever. The hashed
# asset names change with every build; without this the folder would only grow.
echo ">> cleaning ${WWWROOT}"
rm -rf "${WWWROOT}"
mkdir -p "${WWWROOT}"

echo ">> copying ${DX_OUT}/. -> ${WWWROOT}/"
cp -R "${DX_OUT}/." "${WWWROOT}/"

echo ">> done. Commit wwwroot/ with the change that needed it."
