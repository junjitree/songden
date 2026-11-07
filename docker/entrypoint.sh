#!/bin/sh
# YouTube changes often and an old yt-dlp stops working within weeks, so take
# the newest on every start. Offline or failing, the installed one is used.
if [ "$1" != "hash-password" ]; then
    pip install --quiet --no-cache-dir --upgrade "yt-dlp[default]" deno \
        || echo "songden: could not update yt-dlp; using $(yt-dlp --version)" >&2
fi
exec songden "$@"
