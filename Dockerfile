FROM ubuntu:22.04

# The runtime image is `FROM ubuntu:22.04` plus the binary and the browser's bundle. The first thing on
# top of it is a RUNTIME dependency rather than a build one: connected repositories are real clones on a
# volume, and every git command this service runs — clone, fetch, status, commit, push — is this binary.
# Without it the service starts, serves the board, and answers "could not run git: No such file or
# directory" the first time anybody touches a connected repository.
#
# `ca-certificates` for the same reason, and it is not optional either: the remotes are `https://` and
# git verifies them, so a bare image fails every clone on a certificate error instead — a far harder
# failure to read than a missing program.
#
# The base stays ubuntu:22.04, which is what the release workflow builds on: a binary built against a
# newer glibc dies at start-up on a symbol lookup.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY ./target/release/task-manager ./target/release/task-manager

# The client, as `./build-ui.sh` left it in the repository. It is NOT built here and not by the release
# workflow either: `wwwroot/` is committed, so the image carries exactly the bundle that is at the tag.
# The server serves it from `./wwwroot` relative to its working directory, which is `/` in this image —
# see `http_server/ui_middleware.rs`.
COPY ./wwwroot ./wwwroot

ENTRYPOINT ["./target/release/task-manager"]
