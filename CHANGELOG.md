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
  Anywhere on the player bar except its buttons opens the full screen. On a
  phone the bar shows the song on a line above its buttons, or beside the play
  button in simple controls.
- Swiping the player bar up opens the now-playing screen, which follows the
  finger and opens once released past the middle of the screen; Back closes it.
- The screen stays on while music plays (Screen Wake Lock) and may sleep again
  when paused.
- A "Check for new songs" button on the grown-ups' page: re-reads each playlist
  from Spotify in turn and retries songs that were not found, with a check mark
  beside each playlist's name that becomes a loader while it is read (a warning
  if that failed). Spotify is read only then and when a playlist is added; songs
  that finish downloading appear by themselves.
- The app's lion logo in the title bar instead of an emoji.
- The grown-ups' page behind a 5-second hold on a + button: add a Spotify
  playlist by its share link, remove one with a confirming second tap, see how
  many songs of each are ready, with a Downloads card listing each song still on
  its way or not found.
- Settings on the grown-ups' page, kept on the server so every device follows
  them: a song list (tap a playlist to pick a song instead of playing at once),
  shuffle (every song once per round, never repeating across a round's edge;
  previous steps back through what played), and simple controls (only the play
  button).
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
- `FETCH_PARALLEL` sets how many songs are fetched side by side: 1 by default,
  since many parallel requests from one home address trips YouTube's bot checks;
  2-3 for a big first import.
- A search where one result fails to load (an unavailable video) keeps the
  results that did load; it used to drop them all and report the song as not
  found, which lost most of Moana's score.
- A song not found by its title and singer is also searched on YouTube Music by
  its title and album name (from its Spotify page, without any edition after a
  slash), before plain YouTube: covers and lullaby versions crowded out Moana's
  "Know Who You Are", which "Moana Original Motion Picture Soundtrack" finds.
- A song no search finds is looked for on its album: the albums on YouTube Music
  with the name Spotify gives are opened and their tracks checked the same way.
  Bonus tracks like Moana's score demos are listed there even when song searches
  do not bring them up.
- A version the Spotify title asks for must be in the result's title too, so an
  "Instrumental" can't be filled by the sung track of the same length.
- Pressing next while a song is still loading no longer races through the
  playlist: a cancelled load isn't taken for a failed song, and a saved position
  meant for one song can no longer be applied to the next.
- Official uploads that name their artists in the title ("Mark Mancina, Opetaia
  Foa'i - Te Fiti Restored" on a label's channel) are accepted: Spotify's artist
  names are not extra title words, and an artist named in the title counts as
  the singer. Most of Moana's score is only on YouTube this way.
- A shared family password with year-long sessions renewed on use, a global
  limit on wrong passwords, and `SESSIONS_SINCE` to sign every device out.
- Offline play: while the app is open every song is saved on the device, one at
  a time and without being played first, and the grown-ups' page counts them.
  Saved songs are answered in byte ranges and dropped once they leave every
  playlist. Offline, a playlist with no saved songs is dimmed and can't be
  tapped.
- `LISTENER` and `PLAYLIST_PREFIX` settings for the app's name, the title and
  the playlist-name prefix trimmed on tiles.
- `songden hash-password` to hash the family password from stdin.
