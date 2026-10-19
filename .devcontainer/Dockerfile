FROM aruowis-dev-base:1

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates gcc-mingw-w64-x86-64-posix \
        nsis jq zip unzip \
    && rm -rf /var/lib/apt/lists/*
