# Backlog

What is not done, and why. Delete an entry when it ships.

## Not verified yet

- **Nothing has run against the real stack.** `cargo test` covers the parsing,
  matching and ranking (each guarded behaviour was broken once to confirm its
  test fails), but the HTTP routes, the session middleware and the ingester's
  Lidarr/Prowlarr calls have not been exercised end to end. The Python prototype
  these were ported from did run against the same Lidarr and Prowlarr (lookup,
  `release/push`), not this code.
- **On a real iPad:** background play with the screen off, the service-worker
  audio cache (206 slices), the lock and both holds, the iOS 27 status-bar fix
  (needs the app re-added to the home screen to apply).
- **Lidarr `POST /album` for a brand-new album of an existing artist** — the
  body shape (lookup result + artist resource) is from Lidarr's UI, untested
  here.

## Known gaps

- **Version-named songs may never match.** A Spotify title like "How Far I'll
  Go - Alessia Cara Version" only matches a file tagged with the same words. If
  the release tags it plain "How Far I'll Go" (by Alessia Cara), it stays
  "waiting" forever, although the album is downloaded. A fix would compare
  artists when titles differ only by a "<artist> Version" tail.
- **Explicit songs are not filtered.** Spotify marks them (`explicit` is parsed)
  but nothing acts on it. Wanted when other people can edit a playlist.
- **A wrong family password is the only thing between the internet and the
  grown-ups' page.** Deliberate: a PIN screen would be a dead end for the
  listener. Revisit if a second, separate grown-up secret is ever needed.
- **Song pages are read one at a time with a pause**, so a 65-song playlist
  takes about a minute before its albums are known. Fine once; slow for a big
  first import.

## Considered and declined

- **Discovering playlists from a Spotify profile** (anything named "<Listener>
  …"): the public profile page lists only the newest 10 public playlists, and
  the Web API endpoint for a user's playlists was removed in February 2026.
  Links only.
- **Lidarr's Spotify import list**: it renews its Spotify token through
  `spotify.lidarr.audio`, which hung on renewal here, and Lidarr's Spotify lists
  have been switched off upstream before (2025). songden reads Spotify itself
  and talks to Lidarr's normal API.
- **A Spotify developer app**: since February 2026 it needs the owner on
  Premium, caps users at five, and lost the endpoints this would need.
- **Spotify playback in the page**: the Web Playback SDK does not support mobile
  browsers.
