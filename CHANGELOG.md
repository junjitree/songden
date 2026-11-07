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
- Swiping the player bar up opens the now-playing screen, which follows the
  finger and opens once released past the middle of the screen; Back closes it.
- The screen stays on while music plays (Screen Wake Lock) and may sleep again
  when paused.
- A "Check for new songs" button on the grown-ups' page: re-reads every playlist
  from Spotify at once and retries songs that were not found. Spotify is read
  only then and when a playlist is added; songs that finish downloading appear
  by themselves.
- The app's lion logo in the title bar instead of an emoji.
- The grown-ups' page behind a 5-second hold on a + button: add a Spotify
  playlist by its share link, remove one with a confirming second tap, see how
  many songs of each are ready, with a Downloads card listing each song still on
  its way or not found. Returns to the playlists after 30 seconds untouched.
- Playlists read from Spotify's public pages (no login, no developer app), with
  each song's length.
- Songs fetched one at a time from YouTube Music (then YouTube) with yt-dlp and
  kept on the server as `<spotify id>.m4a` with the cover from the song's
  Spotify page. A result counts only if its length is within a few seconds of
  Spotify's, most of the title's telling words agree, and it isn't a version the
  Spotify title didn't ask for (live, cover, karaoke, sped up, reprise…). The
  container carries yt-dlp, Deno and ffmpeg and updates yt-dlp on start.
- A result must be sung by one of Spotify's artists (not counting labels like
  "Disney"), and language words ("Tagalog", "Spanish"…) count as a version the
  title must ask for: dubs run the same length and can keep the title. What was
  used for each song is kept beside it as `<id>.source.json`.
- Downloaded songs carry their title, artists, album, year and cover in the file
  (written with ffmpeg), so the folder works as a Jellyfin music library too.
  Songs fetched before this are tagged on the next "Check for new songs".
- Up to three songs are fetched at once (deliberately few: many parallel
  requests from one home address trips YouTube's bot checks).
- A shared family password with year-long sessions renewed on use, a global
  limit on wrong passwords, and `SESSIONS_SINCE` to sign every device out.
- Offline play: songs are saved on the device after their first play and
  answered in byte ranges, and dropped once they leave every playlist.
- `LISTENER` and `PLAYLIST_PREFIX` settings for the app's name, the title and
  the playlist-name prefix trimmed on tiles.
- `songden hash-password` to hash the family password from stdin.
