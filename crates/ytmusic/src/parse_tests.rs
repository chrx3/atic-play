//! Parser tests against small, real captures of the live endpoints (`tests/fixtures`).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Item;

use super::*;

fn fixture(name: &str) -> Value {
    let raw = match name {
        "search" => include_str!("../tests/fixtures/search.json"),
        "home" => include_str!("../tests/fixtures/home.json"),
        "album" => include_str!("../tests/fixtures/album.json"),
        "artist" => include_str!("../tests/fixtures/artist.json"),
        "playlist" => include_str!("../tests/fixtures/playlist.json"),
        "next" => include_str!("../tests/fixtures/next.json"),
        "player" => include_str!("../tests/fixtures/player.json"),
        "suggestions" => include_str!("../tests/fixtures/suggestions.json"),
        "account" => include_str!("../tests/fixtures/account.json"),
        "library_songs" => include_str!("../tests/fixtures/library_songs.json"),
        "library_songs_next" => include_str!("../tests/fixtures/library_songs_next.json"),
        "library_playlists" => include_str!("../tests/fixtures/library_playlists.json"),
        "library_albums" => include_str!("../tests/fixtures/library_albums.json"),
        "library_artists" => include_str!("../tests/fixtures/library_artists.json"),
        "history" => include_str!("../tests/fixtures/history.json"),
        "library_signed_out" => include_str!("../tests/fixtures/library_signed_out.json"),
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(raw).expect("fixture is valid JSON")
}

// --- helpers --------------------------------------------------------------------------------

#[test]
fn duration_text_is_read_as_seconds() {
    assert_eq!(parse_duration("4:36"), Some(Duration::from_secs(276)));
    assert_eq!(parse_duration("1:02:03"), Some(Duration::from_secs(3723)));
    assert_eq!(parse_duration("0:07"), Some(Duration::from_secs(7)));
    assert_eq!(parse_duration(" 2:54 "), Some(Duration::from_secs(174)));
    assert_eq!(parse_duration("1921"), None);
    assert_eq!(parse_duration("3:99"), None);
    assert_eq!(parse_duration("Daft Punk"), None);
    assert_eq!(parse_duration(""), None);
}

#[test]
fn query_parameters_are_read_without_a_url_crate() {
    let url = "https://rr1---sn-x.googlevideo.com/videoplayback?expire=1&clen=5983881&itag=140";
    assert_eq!(query_param(url, "clen"), Some("5983881"));
    assert_eq!(query_param(url, "itag"), Some("140"));
    assert_eq!(query_param(url, "nope"), None);
    assert_eq!(
        query_param("https://example.com/videoplayback", "clen"),
        None
    );
}

#[test]
fn artists_are_split_out_of_a_plain_byline() {
    let runs = vec![Run {
        text: "Daft Punk, Pharrell Williams y Nile Rodgers".to_string(),
        nav: None,
    }];
    let names: Vec<String> = artists_from_text(&runs)
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["Daft Punk", "Pharrell Williams", "Nile Rodgers"]);
}

#[test]
fn a_leading_type_marker_is_not_an_artist() {
    let runs = vec![
        Run {
            text: "Canción".to_string(),
            nav: None,
        },
        Run {
            text: " • ".to_string(),
            nav: None,
        },
        Run {
            text: "Daft Punk".to_string(),
            nav: None,
        },
        Run {
            text: " • ".to_string(),
            nav: None,
        },
        Run {
            text: "2:54".to_string(),
            nav: None,
        },
    ];
    let names: Vec<String> = artists_from_text(&runs)
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["Daft Punk"]);
}

// --- search ---------------------------------------------------------------------------------

#[test]
fn search_returns_songs_albums_artists_and_playlists() {
    let results = search_results(&fixture("search"));

    assert_eq!(
        results.songs.len(),
        3,
        "two songs from the card and one from the list"
    );
    assert_eq!(results.albums.len(), 1);
    assert_eq!(
        results.artists.len(),
        2,
        "the card artist and the list artist"
    );
    assert_eq!(results.playlists.len(), 1);

    let teachers = &results.songs[0];
    assert_eq!(teachers.video_id, "tuoSS5o4CSI");
    assert_eq!(teachers.title, "Teachers");
    assert_eq!(teachers.duration, Some(Duration::from_secs(174)));
    assert_eq!(teachers.thumbnails.len(), 2);
    assert_eq!(teachers.thumbnails[0].width, 60);
    assert!(teachers.thumbnails[1].width > teachers.thumbnails[0].width);
    assert!(teachers.artists.is_empty(), "the card song has no byline");

    let one_more_time = results
        .songs
        .iter()
        .find(|song| song.video_id == "wU26xVT_vBU")
        .expect("the list song is there");
    assert_eq!(one_more_time.artists.len(), 1);
    assert_eq!(one_more_time.artists[0].name, "Daft Punk");
    assert_eq!(
        one_more_time.artists[0].id.as_deref(),
        Some("UCRr1xG_2WIDs18a6cIiCxeA")
    );

    let album = &results.albums[0];
    assert_eq!(album.browse_id, "MPREb_K8qWMWVqXGi");
    assert_eq!(album.title, "Random Access Memories");
    assert_eq!(album.year, Some(2013));
    assert_eq!(album.artists.len(), 1);
    assert_eq!(album.artists[0].name, "Daft Punk");
    assert_eq!(album.thumbnails.len(), 4);

    let artist = &results.artists[0];
    assert_eq!(artist.browse_id, "UCRr1xG_2WIDs18a6cIiCxeA");
    assert_eq!(artist.name, "Daft Punk");

    let playlist = &results.playlists[0];
    assert_eq!(playlist.id, "PLGfGJFH5ltY8", "the VL prefix is stripped");
    assert_eq!(playlist.title, "daft punk");
    assert_eq!(playlist.author.as_deref(), Some("Rośliniarz"));
}

#[test]
fn suggestions_keep_their_order() {
    let suggestions = suggestions(&fixture("suggestions"));
    assert_eq!(suggestions.len(), 3);
    assert_eq!(suggestions[0], "daft punk");
}

// --- home -----------------------------------------------------------------------------------

#[test]
fn home_returns_titled_shelves_with_items() {
    let shelves = home_shelves(&fixture("home"));
    assert_eq!(shelves.len(), 2);
    assert_eq!(shelves[0].title, "Melodías alegres");
    assert_eq!(shelves[0].items.len(), 3);
    assert_eq!(shelves[1].title, "Sube el volumen");

    match &shelves[0].items[0] {
        Item::Playlist(playlist) => {
            assert_eq!(playlist.title, "Pop Alegre de Hoy");
            assert_eq!(playlist.id, "RDCLAK5uy_la_vISXt1T4mKZhhaYKuLiig6JC21JvqY");
            assert_eq!(playlist.thumbnails.len(), 2);
        }
        other => panic!("expected a playlist, got {other:?}"),
    }
}

// --- album ----------------------------------------------------------------------------------

#[test]
fn album_page_reads_header_and_tracks() {
    let page = album_page(&fixture("album"), "MPREb_K8qWMWVqXGi").expect("album parses");

    assert_eq!(page.album.title, "Random Access Memories");
    assert_eq!(page.album.browse_id, "MPREb_K8qWMWVqXGi");
    assert_eq!(page.album.year, Some(2013));
    assert_eq!(page.album.artists.len(), 1);
    assert_eq!(page.album.artists[0].name, "Daft Punk");
    assert_eq!(
        page.album.artists[0].id.as_deref(),
        Some("UCRr1xG_2WIDs18a6cIiCxeA")
    );
    assert_eq!(page.album.thumbnails.len(), 4);

    assert_eq!(page.tracks.len(), 4, "the related carousel is not a track");
    let first = &page.tracks[0];
    assert_eq!(first.video_id, "IluRBvnYMoY");
    assert_eq!(first.title, "Give Life Back to Music");
    assert_eq!(first.duration, Some(Duration::from_secs(276)));
    assert_eq!(first.artists[0].name, "Daft Punk");
    assert_eq!(
        first.artists[0].id.as_deref(),
        Some("UCRr1xG_2WIDs18a6cIiCxeA")
    );
    assert!(!first.explicit);
}

// --- artist ---------------------------------------------------------------------------------

#[test]
fn artist_page_reads_header_top_songs_and_albums() {
    let page = artist_page(&fixture("artist"), "UCRr1xG_2WIDs18a6cIiCxeA").expect("artist parses");

    assert_eq!(page.artist.name, "Daft Punk");
    assert_eq!(page.artist.browse_id, "UCRr1xG_2WIDs18a6cIiCxeA");
    assert_eq!(page.artist.thumbnails.len(), 3);

    assert_eq!(page.top_songs.len(), 3);
    let song = &page.top_songs[0];
    assert_eq!(song.title, "Instant Crush (feat. Julian Casablancas)");
    assert_eq!(song.video_id, "khnokW3Mw24");
    assert_eq!(song.artists.len(), 2);
    assert_eq!(song.artists[1].name, "Julian Casablancas");
    assert_eq!(
        song.album.as_ref().map(|album| album.id.as_str()),
        Some("MPREb_K8qWMWVqXGi")
    );

    assert_eq!(page.albums.len(), 2);
    let album = &page.albums[0];
    assert_eq!(album.title, "GLBTM (Studio Outtakes)");
    assert_eq!(album.browse_id, "MPREb_X1DQ1j0PPrX");
    assert_eq!(album.year, Some(2023));
    assert!(
        album.artists.is_empty(),
        "the subtitle only has 'Single • 2023'"
    );
}

// --- playlist -------------------------------------------------------------------------------

#[test]
fn playlist_page_returns_its_tracks() {
    let songs = playlist_songs(&fixture("playlist"));
    assert_eq!(songs.len(), 3);
    assert_eq!(songs[0].title, "Instant Crush (feat. Julian Casablancas)");
    assert_eq!(songs[0].duration, Some(Duration::from_secs(338)));
    assert_eq!(songs[0].artists.len(), 2);
    assert_eq!(songs[0].artists[0].name, "Daft Punk");
    assert_eq!(
        songs[0].album.as_ref().map(|album| album.title.as_str()),
        Some("Random Access Memories")
    );
    assert_eq!(songs[2].title, "Voyager");
}

// --- queue ----------------------------------------------------------------------------------

#[test]
fn queue_keeps_the_order_and_reads_the_byline() {
    let songs = queue_songs(&fixture("next"));
    assert_eq!(songs.len(), 3);
    assert_eq!(songs[0].video_id, "4D7u5KF7SP8");
    assert_eq!(
        songs[0].title,
        "Get Lucky (feat. Pharrell Williams and Nile Rodgers)"
    );
    assert_eq!(songs[0].duration, Some(Duration::from_secs(370)));
    assert_eq!(songs[0].artists.len(), 3);
    assert_eq!(songs[0].artists[0].name, "Daft Punk");
    assert_eq!(
        songs[0].artists[0].id.as_deref(),
        Some("UCRr1xG_2WIDs18a6cIiCxeA")
    );
    assert_eq!(
        songs[0].album.as_ref().map(|album| album.title.as_str()),
        Some("Random Access Memories")
    );
    assert_eq!(songs[2].video_id, "dJqxQzxLbHM");
}

// --- stream ---------------------------------------------------------------------------------

#[test]
fn stream_picks_the_best_aac_track() {
    let stream =
        stream_from_player(&fixture("player"), "test-agent").expect("the player response plays");

    assert!(stream.url.starts_with("https://"));
    assert!(stream.url.contains("itag=140"));
    assert_eq!(stream.mime_type, r#"audio/mp4; codecs="mp4a.40.2""#);
    assert_eq!(stream.bitrate, 137_064);
    assert_eq!(stream.content_length, Some(5_983_881));
    assert_eq!(stream.duration, Some(Duration::from_secs(370)));
    let loudness = stream.loudness_db.expect("loudness is there");
    assert!((loudness - 3.1).abs() < 0.05, "got {loudness}");
    assert_eq!(
        stream.headers,
        [("User-Agent".to_string(), "test-agent".to_string())]
    );
    assert!(stream.expires_at > Instant::now());
}

#[test]
fn a_login_required_answer_is_not_playable() {
    let answer = json!({
        "playabilityStatus": {
            "status": "LOGIN_REQUIRED",
            "reason": "Sign in to confirm you are not a bot"
        }
    });
    let reason = stream_from_player(&answer, "test-agent").unwrap_err();
    assert_eq!(reason, "Sign in to confirm you are not a bot");
}

#[test]
fn a_ciphered_answer_is_not_playable() {
    let answer = json!({
        "playabilityStatus": {"status": "OK"},
        "streamingData": {
            "adaptiveFormats": [
                {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 1,
                 "signatureCipher": "s=abc&url=https%3A%2F%2Fexample.com"}
            ]
        }
    });
    let reason = stream_from_player(&answer, "test-agent").unwrap_err();
    assert!(reason.contains("ciphered"), "got: {reason}");
}

#[test]
fn opus_only_audio_is_rejected_for_now() {
    let answer = json!({
        "playabilityStatus": {"status": "OK"},
        "streamingData": {
            "adaptiveFormats": [
                {"itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 146460,
                 "url": "https://example.com/audio?itag=251"}
            ]
        }
    });
    let reason = stream_from_player(&answer, "test-agent").unwrap_err();
    assert!(reason.contains("AAC"), "got: {reason}");
}

#[test]
fn auto_dubbed_tracks_are_skipped() {
    let answer = json!({
        "playabilityStatus": {"status": "OK"},
        "streamingData": {
            "adaptiveFormats": [
                {"itag": 139, "mimeType": "audio/mp4; codecs=\"mp4a.40.5\"", "bitrate": 50_000,
                 "url": "https://example.com/audio?itag=139",
                 "audioTrack": {"audioIsDefault": false, "id": "es-US"}},
                {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 137_000,
                 "url": "https://example.com/audio?itag=140",
                 "audioTrack": {"audioIsDefault": true, "id": "en-US"}}
            ]
        }
    });
    let stream = stream_from_player(&answer, "test-agent").expect("the original plays");
    assert_eq!(stream.bitrate, 137_000);
}

#[test]
fn content_length_can_come_from_the_url() {
    let answer = json!({
        "playabilityStatus": {"status": "OK"},
        "streamingData": {
            "adaptiveFormats": [
                {"itag": 140, "mimeType": "audio/mp4", "bitrate": 137_000,
                 "url": "https://example.com/audio?clen=123456&itag=140"}
            ]
        }
    });
    let stream = stream_from_player(&answer, "test-agent").expect("the format plays");
    assert_eq!(stream.content_length, Some(123_456));
}

// --- account and library ---------------------------------------------------------------------

#[test]
fn the_account_is_read_from_the_menu() {
    let account = account(&fixture("account")).expect("the menu has an account");

    assert_eq!(account.name, "Cuenta de Prueba");
    assert_eq!(account.handle.as_deref(), Some("@cuenta-de-prueba"));
    assert_eq!(account.photo.len(), 2);
    assert_eq!(account.photo[1].width, 176);
}

#[test]
fn a_signed_out_account_menu_has_no_account() {
    let root = json!({
        "actions": [
            { "openPopupAction": { "popup": { "multiPageMenuRenderer": { "sections": [] } } } }
        ]
    });
    assert!(account(&root).is_none());
}

#[test]
fn the_account_handle_falls_back_to_the_byline() {
    let root = json!({
        "actions": [
            {
                "openPopupAction": {
                    "popup": {
                        "multiPageMenuRenderer": {
                            "header": {
                                "activeAccountHeaderRenderer": {
                                    "accountName": { "simpleText": "Cuenta" },
                                    "accountByline": {
                                        "runs": [{ "text": "anon@example.com" }]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        ]
    });

    let account = account(&root).expect("the account is there");
    assert_eq!(account.handle.as_deref(), Some("anon@example.com"));
    assert!(account.photo.is_empty());
}

#[test]
fn the_liked_songs_page_lists_songs_and_its_continuation() {
    let root = fixture("library_songs");
    let songs = songs_in_page(&root);

    assert_eq!(songs.len(), 2);
    assert_eq!(songs[0].video_id, "liked000001");
    assert_eq!(songs[0].title, "Canción de Prueba");
    assert_eq!(songs[0].artists[0].name, "Artista de Prueba");
    assert_eq!(
        songs[0].album.as_ref().map(|album| album.id.as_str()),
        Some("MPREb_testalbum0000001")
    );
    assert_eq!(songs[0].duration, Some(Duration::from_secs(201)));
    assert_eq!(
        continuation(&root).as_deref(),
        Some("ANON_LIKED_SONGS_PAGE_2")
    );
    assert!(!needs_sign_in(&root));
}

#[test]
fn a_continuation_page_lists_its_songs_and_the_next_token() {
    let root = fixture("library_songs_next");
    let songs = songs_in_page(&root);

    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].video_id, "liked000003");
    assert_eq!(songs[0].title, "Tercera Canción");
    assert_eq!(
        continuation(&root).as_deref(),
        Some("ANON_LIKED_SONGS_PAGE_3")
    );
}

#[test]
fn library_playlists_are_read_without_the_vl_prefix() {
    let root = fixture("library_playlists");
    let playlists = playlists_in_page(&root);

    assert_eq!(playlists.len(), 2);
    assert_eq!(playlists[0].id, "PLtest00000000000000000000001");
    assert_eq!(playlists[0].title, "Lista de Prueba");
    assert_eq!(playlists[0].author.as_deref(), Some("Tú"));
    assert_eq!(
        continuation(&root).as_deref(),
        Some("ANON_PLAYLISTS_PAGE_2")
    );
}

#[test]
fn library_albums_are_read_with_year_and_artist() {
    let albums = albums_in_page(&fixture("library_albums"));

    assert_eq!(albums.len(), 2);
    assert_eq!(albums[0].browse_id, "MPREb_testalbum0000001");
    assert_eq!(albums[0].title, "Álbum de Prueba");
    assert_eq!(albums[0].year, Some(2021));
    assert_eq!(albums[0].artists[0].name, "Artista de Prueba");
}

#[test]
fn library_artists_are_read() {
    let artists = artists_in_page(&fixture("library_artists"));

    assert_eq!(artists.len(), 2);
    assert_eq!(artists[0].browse_id, "UCartist0000000000000001");
    assert_eq!(artists[0].name, "Artista de Prueba");
    assert_eq!(artists[0].thumbnails.len(), 1);
}

#[test]
fn a_list_item_can_take_its_page_from_the_title_run() {
    let item = json!({
        "flexColumns": [
            {
                "musicResponsiveListItemFlexColumnRenderer": {
                    "text": {
                        "runs": [
                            {
                                "text": "Artista de Prueba",
                                "navigationEndpoint": {
                                    "browseEndpoint": {
                                        "browseId": "UCfromtitlerun",
                                        "browseEndpointContextSupportedConfigs": {
                                            "browseEndpointContextMusicConfig": {
                                                "pageType": "MUSIC_PAGE_TYPE_ARTIST"
                                            }
                                        }
                                    }
                                }
                            }
                        ]
                    }
                }
            },
            {
                "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": [{ "text": "Artista" }] }
                }
            }
        ]
    });

    let artist = artist_from_list_item(&item).expect("the title run names the page");
    assert_eq!(artist.browse_id, "UCfromtitlerun");
    assert_eq!(artist.name, "Artista de Prueba");
}

#[test]
fn history_keeps_the_shelf_order() {
    let songs = songs_in_page(&fixture("history"));
    let ids: Vec<&str> = songs.iter().map(|song| song.video_id.as_str()).collect();

    assert_eq!(ids, ["history0001", "history0002"]);
    assert_eq!(songs[0].duration, Some(Duration::from_secs(185)));
}

#[test]
fn a_signed_out_library_page_asks_for_a_session() {
    let root = fixture("library_signed_out");

    assert!(needs_sign_in(&root));
    assert!(songs_in_page(&root).is_empty());
    assert!(page_items(&root).is_empty());
}

#[test]
fn created_playlists_and_edit_statuses_are_read() {
    assert_eq!(
        created_playlist_id(&json!({ "playlistId": "VLPLanon" })).unwrap(),
        "PLanon"
    );
    assert_eq!(
        created_playlist_id(&json!({ "playlistId": "PLanon" })).unwrap(),
        "PLanon"
    );
    assert!(created_playlist_id(&json!({})).is_err());
    assert!(created_playlist_id(&json!({ "playlistId": "" })).is_err());

    assert!(playlist_edit_status(&json!({ "status": "STATUS_SUCCEEDED" })).is_ok());
    assert!(playlist_edit_status(&json!({})).is_ok());
    assert!(playlist_edit_status(&json!({ "status": "STATUS_FAILED" })).is_err());
}
