# songden

A self-hosted, kid-safe music player inspired by Yoto and Toniebox: curated
Spotify playlists, played from your own Jellyfin library.

**songden** is a calm, locked-down music player for one small listener. It takes
the idea behind screen-light kids' players like Yoto and Toniebox — a few big,
obvious controls, music a grown-up picked, nothing to wander off into — and
builds it from what a homelab already has: playlists curated in Spotify, songs
stored in your own Jellyfin library, and an iPad or iPhone running it as a
home-screen app.

_Yoto and Toniebox are trademarks of their respective owners. songden is an
independent project and is not affiliated with or endorsed by either._

## What the listener sees

- Big playlist tiles, each a Spotify playlist a grown-up chose.
- A playlist's songs as a list; tap one to play it.
- A player bar: previous, a big lion play button, next.
- A lock: one tap and the screen ignores every touch while the music plays on.
  Unlocking needs the lock held for 3 seconds.

Nothing on the listener's side asks them to type, choose or remember anything.
There is no PIN pad and no login screen in their path: songden was built for a
child for whom a screen like that is a dead end. Grown-up controls sit behind
long holds instead, and lead back on their own.

## What grown-ups get

- **Playlists by link.** Hold the badge for 5 seconds to open the grown-ups'
  page, paste a Spotify playlist's share link, done. Remove one with a
  confirming second tap. The page goes back to the playlists by itself after 30
  seconds untouched.
- **Songs download themselves.** Songs that are not in Jellyfin yet are looked
  up and handed to Lidarr. When Lidarr's own search finds nothing (soundtracks
  often: their catalogue names never match how releases are named), songden
  searches your indexers through Prowlarr by Spotify's album name and offers the
  best few releases to Lidarr, which still checks a release is that album before
  importing it.
- **One family password.** A device signs in once and stays signed in for a
  year, renewed whenever it is used.
- **Offline play.** Songs are saved on the device after their first play.

## How it works

```
Spotify (public pages)      Jellyfin           Lidarr + Prowlarr
  playlist + song pages      the songs           downloads missing albums
          \                     |                     /
           +------------- songden server ------------+
                                |
                     the page on the device
```

- **No Spotify login and no developer app.** Playlists are read from Spotify's
  public embed pages, and each song's album from its public song page. Playlists
  must be public. Spotify's embed page lists at most 100 songs.
- **The device only ever talks to songden.** songden holds the Jellyfin, Lidarr
  and Prowlarr keys and serves only songs that are on a current playlist.
- **Nothing runs on a timer.** Playlists are re-read when the app asks for them
  (at most every 5 minutes), and missing songs are fetched then.

## Running it

You need Jellyfin with a music library. Lidarr (and Prowlarr) are only needed
for the automatic downloads; Lidarr's root folder should be the folder that
Jellyfin library reads.

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
