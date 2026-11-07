# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The player: playlist tiles, each playlist's songs as a tappable list, and a
  player bar with previous / play-pause / next. The play button is a lion; the
  whole lion is the tap target.
- A screen lock that swallows every touch while music plays; held 3 seconds to
  unlock. It survives closing and reopening the app.
- The grown-ups' page behind a 5-second hold on the badge: add a Spotify
  playlist by its share link, remove one with a confirming second tap, see what
  is downloaded and what is still waiting. Returns to the playlists after 30
  seconds untouched.
- Playlists read from Spotify's public pages (no login, no developer app) and
  matched to songs in Jellyfin by title, credit tails ignored and versions
  (reprise, instrumental) kept apart.
- Automatic downloads of missing songs through Lidarr, with a Prowlarr fallback
  for albums Lidarr's own search cannot find.
- A shared family password with year-long sessions renewed on use, a global
  limit on wrong passwords, and `SESSIONS_SINCE` to sign every device out.
- Offline play: songs are saved on the device after their first play and
  answered in byte ranges, and dropped once they leave every playlist.
- `LISTENER` and `PLAYLIST_PREFIX` settings for the app's name, the badge and
  the playlist-name prefix trimmed on tiles.
- `songden hash-password` to hash the family password from stdin.
