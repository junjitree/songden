# songden

A self-hosted, kid-safe music player inspired by Yoto and Toniebox: curated
Spotify playlists, with every song kept on your own server.

**songden** is a calm, locked-down music player for one small listener. It takes
the idea behind screen-light kids' players like Yoto and Toniebox — a few big,
obvious controls, music a grown-up picked, nothing to wander off into — and
builds it from what a homelab already has: playlists curated in Spotify, each
song fetched once onto your own server, and an iPad or iPhone running it as a
home-screen app.

_Yoto and Toniebox are trademarks of their respective owners. songden is an
independent project and is not affiliated with or endorsed by either._

## What the listener sees

- Big playlist tiles, each a Spotify playlist a grown-up chose. Tapping one
  plays it straight away, picking up where that playlist was left.
- A now-playing screen: the cover, the song, and previous / a big lion play
  button / next. Back returns to the tiles.
- While something plays, a small player bar on the tiles.

Nothing on the listener's side asks them to type, choose or remember anything.
There is no PIN pad and no login screen in their path: songden was built for a
child for whom a screen like that is a dead end. The grown-ups' page sits behind
a long hold instead.

## What grown-ups get

- **Playlists by link.** Hold the **+** for 5 seconds to open the grown-ups'
  page (a tap does nothing), paste a Spotify playlist's share link, done. Remove
  one with a confirming second tap.
- **Settings** on the same page, the same on every device: a **song list**
  (tapping a playlist shows its songs to pick from, instead of playing at once),
  **shuffle** (every song once before any repeats), and **simple controls**
  (only the play button, no previous or next).
- **Songs fetch themselves, one by one.** Each song on a playlist is looked up
  on YouTube Music (then YouTube) with yt-dlp and saved on the server. A result
  only counts if its length is within a few seconds of Spotify's and it isn't a
  version the playlist didn't ask for (live, cover, karaoke, sped up…), so the
  listener gets the recording they know. Songs not found show on the grown-ups'
  page and are retried when asked.
- **One family password.** A device signs in once and stays signed in for a
  year, renewed whenever it is used.
- **Offline play.** While the app is open, every song is saved on the device,
  one at a time, without having to be played first. The grown-ups' page shows
  how many are saved.

## How it works

```
Spotify (public pages)            YouTube (yt-dlp)
  which songs, their lengths        the audio, fetched once
            \                         /
             +---- songden server ----+   songs kept on disk
                         |
              the page on the device
```

- **No Spotify login and no developer app.** Playlists are read from Spotify's
  public embed pages, and each song's cover from its public song page. Playlists
  must be public. Spotify's embed page lists at most 100 songs.
- **The device only ever talks to songden,** which serves only songs that are on
  a current playlist.
- **Spotify is read only when a grown-up asks:** adding a playlist, or "Check
  for new songs" on the grown-ups' page. The listener's playlists never change
  under them by themselves; songs that finish downloading simply appear. Nothing
  runs on a timer.
- **Audio** is what YouTube serves, usually AAC around 130 kbps: plenty for a
  tablet speaker or headphones. yt-dlp is updated each time the container
  starts, because YouTube regularly breaks older versions.

## Running it

Docker, and a disk for the songs (a few MB each). The image carries yt-dlp, Deno
and ffmpeg.

1. `cp .env.example .env`, fill it in, `chmod 600 .env`.
2. Hash the family password and put the hash in `.env`:
   `echo -n 'the password' | docker compose -f compose.example.yml run --rm -T songden hash-password`
3. `docker compose -f compose.example.yml up -d --build`
4. Put it behind your HTTPS reverse proxy.
5. On the iPad or iPhone: open it in Safari, sign in, Share → Add to Home
   Screen.

To keep the listener inside the app, use the device's own lock: Guided Access on
iPhone and iPad, app pinning on Android.

### Exposing it to the internet

The family password guards everything except the page itself. Wrong passwords
are limited for everyone at once (5 in 15 minutes, then sign-in pauses); devices
already signed in are unaffected. Add fail2ban (or similar) on `POST /api/login`
answering 400/429 at your proxy for more.

## Developing

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

`tools/make_icons.py` redraws the app icons (standard library only).

## Licence

MIT. See [LICENSE](LICENSE).
