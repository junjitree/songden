# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The player: playlist tiles that play as soon as they are tapped (resuming
  where each playlist was left), a now-playing screen, and a player bar on the
  tiles, each with previous / play-pause / next. The play button is a lion; the
  whole lion is the tap target.
- Seeking on the now-playing screen: a thick track with a large handle and
  elapsed/total times. The controls never move: the title always takes two lines
  (longer ones end in an ellipsis), the artist one, the times a fixed width.
  Anywhere on the player bar except its buttons opens the full screen.
- The screen stays on while music plays (Screen Wake Lock) and may sleep again
  when paused.
- A "Check for new songs" button on the grown-ups' page: re-reads every playlist
  from Spotify at once (nothing runs on a timer) and retries albums that were
  not found, without waiting out the six-hour retry gap.
- The grown-ups' page behind a 5-second hold on a + button: add a Spotify
  playlist by its share link, remove one with a confirming second tap, see what
  is downloaded and what is still waiting, with a Downloads card listing each
  album still on its way. Returns to the playlists after 30 seconds untouched.
- Playlists read from Spotify's public pages (no login, no developer app) and
  matched to songs in Jellyfin by title, credit tails ignored and versions
  (reprise, instrumental) kept apart.
- Automatic downloads of missing songs through Lidarr, with a Prowlarr fallback
  for albums Lidarr's own search cannot find. Downloads are MP3, standard
  edition first: the fallback never offers lossless releases, and artists
  songden adds get the `LIDARR_QUALITY_PROFILE` profile (default "Standard").
  Releases are offered under a name carrying a quality Lidarr recognises
  (MP3-320), and Prowlarr's magnet-redirect links go to Lidarr as download links
  so the download client actually receives them.
- A shared family password with year-long sessions renewed on use, a global
  limit on wrong passwords, and `SESSIONS_SINCE` to sign every device out.
- Offline play: songs are saved on the device after their first play and
  answered in byte ranges, and dropped once they leave every playlist.
- `LISTENER` and `PLAYLIST_PREFIX` settings for the app's name, the title and
  the playlist-name prefix trimmed on tiles.
- `songden hash-password` to hash the family password from stdin.
