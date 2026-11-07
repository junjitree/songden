# Backlog

What is not done, and why. Delete an entry when it ships.

## Not verified yet

- **Saving songs for offline has only run in headless Chromium on Linux.**
  `saveSongs` in `index.html` and `save` in `sw.js` were driven there through a
  proxy: a cut download, the server gone, a session ended and a small storage
  quota. Whether iOS keeps the songs (`navigator.storage.persist` is asked, the
  answer is not acted on) and how far a save gets before iOS suspends the app
  are unknown.
- **On a real iPad:** background play with the screen off, the service-worker
  audio cache (206 slices), the 5-second + hold, whether a tile tap starts sound
  right away (iOS only allows play inside a tap; songs are fetched ahead so the
  tap needs no network), the iOS 27 status-bar fix (needs the app re-added to
  the home screen to apply).

## Known gaps

- **yt-dlp depends on YouTube's current behaviour.** Without the newest yt-dlp
  (and Deno beside it) downloads fail with 403; MeTube's 90-day-old copy did.
  The container updates yt-dlp on start; a long-running container can still go
  stale. A restart fixes it. If YouTube starts requiring proof-of-origin tokens
  for these downloads, the bgutil token provider (which MeTube ships) would be
  the next piece to add.
- **Explicit songs are not filtered.** Spotify marks them (`explicit` is parsed)
  but nothing acts on it. Wanted when other people can edit a playlist.
- **A wrong family password is the only thing between the internet and the
  grown-ups' page.** Deliberate: a PIN screen would be a dead end for the
  listener. Revisit if a second, separate grown-up secret is ever needed.
- **Songs are fetched one at a time,** each with a search that looks up every
  result's length (several seconds), so a big new playlist takes a while.
- **State files are world-readable inside the container** (`sessions.json` holds
  live session tokens). Only matters if something else shares the state
  directory; writing them `0600` would close it.
- **A restart drops the fetch queue.** Nothing resumes on its own; the next
  "Check for new songs" (or added playlist) queues what is still missing.

- **A song being retried shows twice in the Downloads card.** During "Check for
  new songs", `parents_status` in `http.rs` lists `Fetcher::status`'s queued
  songs and its failures, and `retry_failed` keeps the failure record until the
  retry ends, so one song appears as both "downloading" and "not found". Seen in
  `/api/parents/status` output; cosmetic, and it clears when the retry ends. The
  fix is to skip failures whose id is queued.

- **At 320px wide the player bar's next button runs off the screen** in normal
  controls (its right edge measured 337px; the same before and after the title
  line). Measured at 320, 360 and 390 wide, and only 320 overflows. Hiding the
  bar's album art at that width would make room.

## Considered and declined

- **Torrents through Lidarr and Prowlarr** (the first design): albums rather
  than songs, so a playlist pulled whole soundtracks and still missed singles,
  compilation-only tracks and demos; soundtrack catalogue names never matched
  release names; Lidarr's metadata server and Spotify lists were unreliable, and
  public indexers carry malware. Replaced by per-song YouTube fetching.
- **Driving MeTube's API instead of bundling yt-dlp:** MeTube only downloads a
  link it is given; it can't search and compare lengths, which is how songden
  picks the right recording, and it would tie songden to MeTube.
- **Discovering playlists from a Spotify profile** (anything named "<Listener>
  …"): the public profile page lists only the newest 10 public playlists, and
  the Web API endpoint for a user's playlists was removed in February 2026.
  Links only.
- **A Spotify developer app**: since February 2026 it needs the owner on
  Premium, caps users at five, and lost the endpoints this would need.
- **Spotify playback in the page**: the Web Playback SDK does not support mobile
  browsers.
