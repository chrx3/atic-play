//! Live tests against music.youtube.com.
//!
//! They need the network and a working YouTube Music session, so they are ignored by default:
//!
//! ```text
//! cargo test -j 4 -p play-ytmusic -- --ignored --nocapture
//! ```

use play_ytmusic::{ArtistRef, Client, Error};

fn client() -> Client {
    Client::new("es", "CL").expect("the anonymous session is created")
}

/// The signed-in client, or `None` when `ATIC_PLAY_COOKIE` is not set (the tests then skip).
fn signed_client() -> Option<Client> {
    let cookies = std::env::var("ATIC_PLAY_COOKIE").ok()?;
    if cookies.trim().is_empty() {
        return None;
    }
    Some(
        Client::with_cookies("es", "CL", &cookies)
            .expect("ATIC_PLAY_COOKIE is a Cookie header with a SAPISID"),
    )
}

fn artist_names(artists: &[ArtistRef]) -> String {
    if artists.is_empty() {
        "(none)".to_string()
    } else {
        artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[test]
#[ignore = "hits the network"]
fn search_finds_daft_punk() {
    let results = client().search("daft punk").expect("search works");

    println!(
        "songs={} albums={} artists={} playlists={}",
        results.songs.len(),
        results.albums.len(),
        results.artists.len(),
        results.playlists.len()
    );
    for song in results.songs.iter().take(5) {
        println!(
            "  {} — {} [{:?}]",
            song.title,
            artist_names(&song.artists),
            song.duration
        );
    }

    assert!(!results.songs.is_empty(), "the search returns songs");
    let songs_with_video = results
        .songs
        .iter()
        .filter(|song| !song.video_id.is_empty())
        .count();
    assert_eq!(songs_with_video, results.songs.len());
    assert!(
        results.songs.iter().any(|song| {
            song.title.to_lowercase().contains("get lucky")
                || song.artists.iter().any(|artist| artist.name == "Daft Punk")
        }),
        "some result is by Daft Punk"
    );
    assert!(
        results
            .artists
            .iter()
            .any(|artist| artist.name.to_lowercase().contains("daft punk")),
        "the artist page is among the results"
    );
}

#[test]
#[ignore = "hits the network"]
fn home_returns_titled_shelves() {
    let shelves = client().home().expect("home works");
    println!("{} shelves", shelves.len());
    for shelf in &shelves {
        println!("  {} ({})", shelf.title, shelf.items.len());
    }
    assert!(!shelves.is_empty(), "the home page has shelves");
    assert!(shelves.iter().all(|shelf| !shelf.title.is_empty()));
    assert!(shelves.iter().any(|shelf| !shelf.items.is_empty()));
}

#[test]
#[ignore = "hits the network"]
fn album_page_has_tracks() {
    let page = client()
        .album("MPREb_K8qWMWVqXGi")
        .expect("the album page works");

    println!(
        "{} — {} [{:?}] ({} tracks)",
        page.album.title,
        artist_names(&page.album.artists),
        page.album.year,
        page.tracks.len()
    );
    for track in page.tracks.iter().take(5) {
        println!("  {} [{:?}]", track.title, track.duration);
    }

    assert!(!page.album.title.is_empty());
    assert!(!page.tracks.is_empty(), "the album lists its tracks");
    assert_eq!(page.album.browse_id, "MPREb_K8qWMWVqXGi");
}

#[test]
#[ignore = "hits the network"]
fn up_next_starts_with_the_song() {
    let songs = client().up_next("4D7u5KF7SP8").expect("the queue works");

    println!("{} songs in the queue", songs.len());
    for song in songs.iter().take(5) {
        println!(
            "  {} — {} [{:?}]",
            song.title,
            artist_names(&song.artists),
            song.duration
        );
    }

    assert!(songs.len() > 1, "a radio has more than the current song");
    assert_eq!(songs[0].video_id, "4D7u5KF7SP8");
}

#[test]
#[ignore = "hits the network"]
fn stream_url_serves_byte_ranges() {
    let stream = client().stream("4D7u5KF7SP8").expect("the song plays");

    println!(
        "{} @ {} kbps, {} bytes, {:?}, loudness {:?}",
        stream.mime_type,
        stream.bitrate / 1000,
        stream
            .content_length
            .map(|length| length.to_string())
            .unwrap_or_else(|| "?".to_string()),
        stream.duration,
        stream.loudness_db
    );
    println!("{}", stream.url);

    assert!(stream.mime_type.starts_with("audio/mp4"));
    assert!(stream.bitrate > 0);
    assert!(stream.url.starts_with("https://"));
    assert!(stream.expires_at > std::time::Instant::now());

    let mut request = ureq::get(&stream.url).header("Range", "bytes=0-1023");
    for (name, value) in &stream.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.call().expect("googlevideo answers");
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .with_config()
        .limit(4096)
        .read_to_vec()
        .expect("the first kilobytes arrive");

    println!("range GET: HTTP {status}, {} bytes", body.len());
    assert_eq!(status, 206);
    assert_eq!(body.len(), 1024);
    assert_eq!(&body[4..8], b"ftyp", "an MP4 starts with a ftyp box");
}

#[test]
#[ignore = "hits the network"]
fn suggestions_typo_completion() {
    let suggestions = client()
        .search_suggestions("daft")
        .expect("suggestions work");
    println!("{suggestions:?}");
    assert!(!suggestions.is_empty());
    assert!(
        suggestions
            .iter()
            .any(|s| s.to_lowercase().contains("daft"))
    );
}

// --- signed-in session -----------------------------------------------------------------------

#[test]
#[ignore = "hits the network; needs ATIC_PLAY_COOKIE with a signed-in Cookie header"]
fn account_is_read_with_a_real_session() {
    let Some(client) = signed_client() else {
        println!("ATIC_PLAY_COOKIE is not set: skipping");
        return;
    };

    assert!(client.is_signed_in());
    let account = client.account().expect("the account menu reads");
    println!(
        "account: {:?} ({:?}), {} photo sizes",
        account.name,
        account.handle,
        account.photo.len()
    );
    assert!(!account.name.is_empty(), "the account has a name");
}

#[test]
#[ignore = "hits the network; needs ATIC_PLAY_COOKIE with a signed-in Cookie header"]
fn library_lists_are_read_with_a_real_session() {
    let Some(client) = signed_client() else {
        println!("ATIC_PLAY_COOKIE is not set: skipping");
        return;
    };

    let songs = client.liked_songs().expect("liked songs read");
    println!("liked songs: {}", songs.len());
    for song in songs.iter().take(3) {
        println!("  {} — {}", song.title, artist_names(&song.artists));
    }

    let playlists = client.library_playlists().expect("library playlists read");
    println!("library playlists: {}", playlists.len());
    for playlist in playlists.iter().take(3) {
        println!("  {} [{}]", playlist.title, playlist.id);
    }

    let albums = client.library_albums().expect("library albums read");
    println!("library albums: {}", albums.len());
    for album in albums.iter().take(3) {
        println!("  {} [{}]", album.title, album.browse_id);
    }

    let artists = client.library_artists().expect("library artists read");
    println!("library artists: {}", artists.len());
    for artist in artists.iter().take(3) {
        println!("  {} [{}]", artist.name, artist.browse_id);
    }
    // They are artists, not whatever else the page may hold: a channel id and a name.
    assert!(
        artists.iter().all(|artist| artist.browse_id.starts_with("UC")),
        "every library artist has a channel browse id"
    );
    assert!(
        artists.iter().all(|artist| !artist.name.trim().is_empty()),
        "every library artist has a name"
    );

    let history = client.history().expect("history reads");
    println!("history: {}", history.len());
    for song in history.iter().take(3) {
        println!("  {} — {}", song.title, artist_names(&song.artists));
    }
}

#[test]
#[ignore = "hits the network (only for the anonymous visitor id)"]
fn edits_need_a_session_before_touching_the_network() {
    let client = client();

    assert!(matches!(client.liked_songs(), Err(Error::SignedOut)));
    assert!(matches!(
        client.set_liked("4D7u5KF7SP8", true),
        Err(Error::SignedOut)
    ));
    assert!(matches!(
        client.add_to_playlist("PLanon", "4D7u5KF7SP8"),
        Err(Error::SignedOut)
    ));
    assert!(matches!(
        client.create_playlist("Una lista", &[]),
        Err(Error::SignedOut)
    ));
}

#[test]
#[ignore = "hits the network"]
fn expired_or_invalid_cookies_report_a_signed_out_session() {
    // The cookie parses, so requests are signed, but YouTube rejects them.
    let client = Client::with_cookies("es", "CL", "SAPISID=not-a-real-cookie")
        .expect("the cookie has a SAPISID");
    assert!(client.is_signed_in());

    assert!(matches!(client.account(), Err(Error::SignedOut)));
    assert!(matches!(client.liked_songs(), Err(Error::SignedOut)));
}
